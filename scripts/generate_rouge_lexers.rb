# frozen_string_literal: true

# The `lexers` command of scripts/generate_rouge.rb: Rouge's lexer classes
# as Rust tables (crates/adoctra-highlight/src/generated/lexers.rs).
#
# Every lexer class is exported with its resolved states (inherited, mixed
# in, prepended/appended), its rules' regexps and actions and the registry
# of tags and aliases. A rule with a token and next state is data. A rule
# with a Ruby block becomes a Rust callback in one of three ways:
#
# * a hand-written `rule_<line>` in src/lexers/<file>.rs (the block's file
#   relative to Rouge's lexers directory, `/` as `_`), which wins;
# * a translation of the block's AST when it only uses the supported subset
#   (token/groups/push/pop!/goto/reset_stack/delegate/recurse, `m[i]`,
#   conditionals over collections, regexps, strings and states); constants
#   and collections are evaluated per lexer class, so an inherited block
#   sees the subclass like `self.class` does in Ruby;
# * otherwise its location is listed in the lexer's `missing` hooks and the
#   lexer is not used.
#
# `start` blocks, `initialize` overrides and custom `stream_tokens` are
# hooks: hand-written `start_<line>`, `init` and `stream` functions.

require 'set'

module RougeLexers
  LEXERS_DIR = File.join(Gem.loaded_specs['rouge'].full_gem_path, 'lib', 'rouge', 'lexers')
  RUST_DIR = File.expand_path('../crates/adoctra-highlight/src/lexers', __dir__)

  # Rule metadata by Rule object, recorded while states are built.
  META = {}.compare_by_identity

  def self.install_hook
    Rouge::RegexLexer::StateDSL.prepend(Module.new do
      define_method(:rule) do |re, tok = nil, next_state = nil, &blk|
        result = super(re, tok, next_state, &blk)
        RougeLexers::META[rules.last] = { token: tok, next: next_state, block: blk }
        result
      end
    end)
  end

  class Unsupported < StandardError; end

  # A Ruby file of Rouge as a Rust module name: relative to the lexers
  # directory, else to `lib/rouge` (e.g. `template_lexer.rb`), never an
  # absolute path that depends on where the gem is installed.
  def self.module_name path
    rel = path.delete_prefix("#{LEXERS_DIR}/").delete_prefix("#{File.dirname LEXERS_DIR}/").delete_suffix('.rb')
    raise "Rouge file outside the gem: #{path}" if rel.start_with?('/')

    rel.tr('/', '_').tr('-', '_')
  end

  def self.rust_ident text
    text.gsub(/[^A-Za-z0-9_]/, '_')
  end

  # The hand-written functions of each Rust lexer module.
  def self.hand_written
    @hand_written ||= Dir[File.join(RUST_DIR, '*.rs')].to_h do |file|
      [File.basename(file, '.rs'), File.read(file).scan(/^\s*pub\(crate\) fn (\w+)/).flatten.to_set]
    end
  end

  def self.hand? path, name
    (hand_written[module_name(path)] || Set.new).include?(name)
  end

  # Lexers a hand-written module delegates to, by lexer tag: its
  # `Delegates (tag, …): dep, ….` doc lines.
  def self.hand_deps path, tag
    file = File.join(RUST_DIR, "#{module_name path}.rs")
    return [] unless File.file?(file)

    File.read(file).scan(/Delegates \(([^)]*)\): ([^.]*)\./).flat_map do |tags, deps|
      tags.split(/,\s*/).include?(tag) ? deps.split(/,\s*/) : []
    end
  end

  class Generator
    attr_reader :regexes, :sets, :callbacks

    # Lexer tags each exported lexer delegates to, by lexer tag.
    attr_reader :deps

    def initialize tokens
      @deps = Hash.new { |hash, key| hash[key] = Set.new }
      @current = nil
      @tokens = tokens
      @regex_index = {}
      @regexes = []
      @sets = {}
      @callbacks = []
      @callback_names = {}
    end

    def regex re
      key = [re.source, re.options & 7]
      @regex_index[key] ||= (@regexes << key).size - 1
    end

    def token_const tok
      # The root `Token` (toml's `rule %r//, Token, :value`) only ever has an
      # empty value, which `continue_lex` drops.
      return 'T::TEXT' if tok == Rouge::Token

      raise Unsupported, "not a token: #{tok.inspect}" unless tok.is_a?(Class) && tok < Rouge::Token

      "T::#{const_name tok.qualname}"
    end

    def const_name qualname
      qualname.gsub('.', '_').gsub(/([a-z])([A-Z])/, '\1_\2').upcase
    end

    # A static sorted string slice for a collection.
    def set_const values
      values = values.map(&:to_s).uniq.sort
      @sets[values] ||= "SET_#{@sets.size}"
    end

    def next_expr nxt, states
      case nxt
      when nil then 'Next::Stay'
      when :pop! then 'Next::Pop'
      when :push then 'Next::Push'
      when Symbol, String
        # Rouge resolves the name when the rule fires: an undefined state
        # raises then, not when the lexer loads.
        states.include?(nxt.to_sym) ? "Next::State(#{state_index states, nxt})" : "Next::Unknown(#{rust_str nxt.to_s})"
      when Array then "Next::Many(&[#{nxt.map { |n| next_expr n, states }.join(', ')}])"
      else raise Unsupported, "next state #{nxt.inspect}"
      end
    end

    def state_index states, name
      states.index(name.to_sym) or raise Unsupported, "unknown state #{name}"
    end

    # Exports one lexer class: [rust item source, missing hooks].
    def depend tag
      @deps[@current] << tag if @current
    end

    def lexer lexer
      @current = lexer.tag
      missing = []
      ident = "LEXER_#{RougeLexers.rust_ident(lexer.tag).upcase}"
      states = []
      regex_lexer = lexer < Rouge::RegexLexer
      if regex_lexer
        # InheritableHash#keys includes the inherited states.
        states.concat(lexer.state_definitions.keys)
      end
      state_sources = states.map do |name|
        begin
          state = lexer.get_state(name)
        rescue StandardError, NotImplementedError => e
          # Only the exception class: messages vary across Ruby versions
          # and carry object addresses.
          missing << "#{lexer.tag}: state #{name}: #{e.class}"
          next %(StateDef { name: #{rust_str name.to_s}, rules: &[] })
        end
        rules = state.rules.map do |rule|
          if rule.is_a?(Rouge::RegexLexer::State)
            "Rule::Mixin(#{state_index states, rule.name})"
          else
            meta = META.fetch(rule)
            bol = rule.beginning_of_line
            action = action_for(lexer, rule, meta, states, missing)
            "Rule::Match { regex: #{regex rule.re}, bol: #{bol}, action: #{action} }"
          end
        end
        %(StateDef { name: #{rust_str name.to_s}, rules: &[\n#{rules.map { |rule| "            #{rule},\n" }.join}        ] })
      end
      start = hooks_start(lexer, missing)
      init = hook_init(lexer, missing)
      reset = hook_method(lexer, :reset!, 'reset', missing, [Rouge::Lexer, Rouge::RegexLexer])
      stream = hook_stream(lexer, missing)
      source = +"static #{ident}: LexerDef = LexerDef {\n"
      source << "    tag: #{rust_str lexer.tag},\n"
      source << "    states: &[\n#{state_sources.map { |state| "        #{state},\n" }.join}    ],\n"
      source << "    start: &[#{start.join(', ')}],\n"
      source << "    init: #{init},\n"
      source << "    reset: #{reset},\n"
      source << "    stream: #{stream},\n"
      source << "    missing: __MISSING__,\n"
      source << "};\n"
      [ident, source, missing]
    end

    def location blk
      path, line = blk.source_location
      ["#{RougeLexers.module_name path}.rs:#{line}", path, line]
    end

    def action_for lexer, rule, meta, states, missing
      blk = meta[:block]
      unless blk
        return "Action::Emit(#{token_const meta[:token]}, #{next_expr meta[:next], states})"
      end

      where, path, line = location(blk)
      if RougeLexers.hand?(path, "rule_#{line}")
        return "Action::Call(crate::lexers::#{RougeLexers.module_name path}::rule_#{line})"
      end

      name = "cb_#{RougeLexers.rust_ident lexer.tag}_#{RougeLexers.rust_ident RougeLexers.module_name(path)}_#{line}"
      unless @callback_names.key?(name)
        @callback_names[name] = begin
          body = Translator.of_block(self, lexer, blk, states, :rule).translate
          @callbacks << "fn #{name}(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {\n#{body}    Ok(())\n}\n"
          true
        rescue Unsupported => e
          missing << "#{where} (#{e.message})"
          false
        end
      end
      if @callback_names[name]
        "Action::Call(#{name})"
      else
        missing << where unless missing.any? { |m| m.start_with?(where) }
        'Action::Call(crate::lexer::unported)'
      end
    end

    def hooks_start lexer, missing
      return [] unless lexer < Rouge::RegexLexer

      lexer.start_procs.to_a.map do |blk|
        where, path, line = location(blk)
        next "crate::lexers::#{RougeLexers.module_name path}::start_#{line}" if RougeLexers.hand?(path, "start_#{line}")

        name = "st_#{RougeLexers.rust_ident lexer.tag}_#{RougeLexers.rust_ident RougeLexers.module_name(path)}_#{line}"
        hook(name, missing, "#{where} (start)") do
          Translator.of_block(self, lexer, blk, lexer.state_definitions.keys, :hook).translate
        end
      end
    end

    # A translated hook function, or the unported placeholder.
    def hook name, missing, where
      unless @callback_names.key?(name)
        @callback_names[name] = begin
          body = yield
          @callbacks << "fn #{name}(l: &mut Lexer) -> Result<(), LexError> {\n#{body}    Ok(())\n}\n"
          true
        rescue Unsupported => e
          missing << "#{where} (#{e.message})"
          false
        end
      end
      return name if @callback_names[name]

      missing << where unless missing.any? { |m| m.start_with?(where) }
      'crate::lexer::unported_hook'
    end

    def owner_file lexer, meth
      lexer.instance_method(meth).source_location&.first
    end

    def hook_init lexer, missing
      owner = lexer.instance_method(:initialize).owner
      return 'None' if [Rouge::Lexer, Rouge::RegexLexer].include?(owner) || !(owner < Rouge::Lexer)

      path = owner_file(lexer, :initialize)
      if RougeLexers.hand?(path, 'init')
        RougeLexers.hand_deps(path, lexer.tag).each { |dep| depend(dep) }
        return "Some(crate::lexers::#{RougeLexers.module_name path}::init)"
      end

      # `initialize` creating delegate lexers after `super`.
      name = "init_#{RougeLexers.rust_ident lexer.tag}"
      fn = hook(name, missing, "#{RougeLexers.module_name path}.rs (initialize)") do
        ast = RubyVM::AbstractSyntaxTree.of(lexer.instance_method(:initialize))
        states = lexer < Rouge::RegexLexer ? lexer.state_definitions.keys : []
        Translator.new(self, owner, ast, states, owner, :hook).translate
      end
      fn == 'crate::lexer::unported_hook' ? 'None' : "Some(#{fn})"
    end

    # An overridden instance method ported as the hand-written `name`.
    def hook_method lexer, meth, name, missing, bases
      owner = lexer.instance_method(meth).owner
      return 'None' if bases.include?(owner)

      path = owner.instance_method(meth).source_location&.first
      return "Some(crate::lexers::#{RougeLexers.module_name path}::#{name})" if path && RougeLexers.hand?(path, name)

      missing << "#{path ? RougeLexers.module_name(path) : owner.name}.rs (#{meth})"
      'None'
    end

    def hook_stream lexer, missing
      owner = lexer.instance_method(:stream_tokens).owner
      return 'None' if owner == Rouge::RegexLexer

      path = owner_file(lexer, :stream_tokens)
      if RougeLexers.hand?(path, 'stream')
        RougeLexers.hand_deps(path, lexer.tag).each { |dep| depend(dep) }
        return "Some(crate::lexers::#{RougeLexers.module_name path}::stream)"
      end

      missing << "#{RougeLexers.module_name path}.rs (stream_tokens)"
      'None'
    end
  end

  # Translates the AST of a rule block (mode :rule, a Rust callback on a
  # `Scan` and the match `m`) or of a `start` block / `initialize` method
  # (mode :hook, on the `Lexer` `l`) into a Rust function body.
  class Translator
    def initialize generator, lexer, ast, states, binding_source, mode
      @g = generator
      @lexer = lexer
      @binding = binding_source
      @states = states
      @mode = mode
      @ast = ast
      raise Unsupported, 'no AST' unless @ast

      @param = @ast.children[0].first if mode == :rule
      args = @ast.children[1]
      raise Unsupported, 'block arity' if mode == :rule && args && args.children[0] > 1

      # Ruby locals by name: :string or :token, declared up front because
      # an assignment in a branch is visible after it.
      @locals = {}
      # Locals bound to constant collections, by name.
      @consts = {}
    end

    def self.of_block generator, lexer, blk, states, mode
      new(generator, lexer, RubyVM::AbstractSyntaxTree.of(blk), states, blk, mode)
    end

    def translate
      body = stmts(@ast.children[2], '    ')
      decls = @locals.reject { |_, kind| kind == :param }.map do |name, kind|
        kind == :token ? "    let mut #{local name}: T::Token = T::TEXT;\n" : "    let mut #{local name} = String::new();\n"
      end
      decls.join + body
    end

    def local name
      "v_#{name}"
    end

    def unsupported node, what = nil
      raise Unsupported, (what || "#{node.type}@#{node.first_lineno}")
    end

    def stmts node, indent
      return '' if node.nil?

      list = node.type == :BLOCK ? node.children : [node]
      list.map { |n| stmt(n, indent) }.join
    end

    def call_args node
      return [] if node.nil?

      unsupported(node) unless node.type == :LIST

      node.children.compact
    end

    def rule?
      @mode == :rule
    end

    def stmt node, indent
      case node.type
      when :FCALL, :VCALL
        name = node.children[0]
        args = node.type == :FCALL ? call_args(node.children[1]) : []
        rule? ? ops(name, args, node, indent) : hook_ops(name, args, node, indent)
      when :IF, :UNLESS
        cond, yes, no = node.children
        # `puts … if @debug`: Rouge's debugging is off.
        if cond.type == :IVAR && cond.children[0] == :@debug
          return stmts(node.type == :IF ? no : yes, indent)
        end
        test = condition(cond)
        test = "!(#{test})" if node.type == :UNLESS
        out = "#{indent}if #{test} {\n#{stmts yes, indent + '    '}#{indent}}"
        out << " else {\n#{stmts no, indent + '    '}#{indent}}" if no
        "#{out}\n"
      when :CASE
        subject = value(node.children[0])
        clauses = []
        branch = node.children[1]
        while branch
          if branch.type == :WHEN
            values, body, branch = branch.children
            tests = call_args(values).map { |v| "#{subject} == #{literal_string v}" }
            clauses << ["(#{tests.join(' || ')})", body]
          else
            clauses << [nil, branch]
            branch = nil
          end
        end
        out = +''
        clauses.each_with_index do |(test, body), i|
          out << (test ? "#{i.zero? ? indent : ' else '}if #{test} {\n" : " else {\n")
          out << stmts(body, indent + '    ') << "#{indent}}"
        end
        "#{out}\n"
      when :DASGN, :LASGN
        unsupported(node, 'assignment in a hook') unless rule?
        name, rhs = node.children
        if !@locals.key?(name) && (constant = constant_value(rhs))
          # `keywords = self.class.keywords`: a constant collection.
          @consts[name] = constant
          return ''
        end
        kind = token_expression?(rhs) ? :token : :string
        if (known = @locals[name]) && known != kind
          unsupported(node, "local #{name} changes type")
        end
        @locals[name] = kind
        expr = kind == :token ? token_expr(rhs) : "(#{value rhs}).to_string()"
        "#{indent}#{local name} = #{expr};\n"
      when :IASGN
        name, rhs = node.children
        if rhs.type == :IVAR
          # `@lang = @css`: one delegate lexer under two names.
          receiver = rule? ? 's.lexer' : 'l'
          return "#{indent}#{receiver}.alias_sub(#{rust_str ivar_key(name)}, #{rust_str ivar_key(rhs.children[0])});\n"
        end
        receiver = rule? ? 's.lexer' : 'l'
        key = rust_str(ivar_key(name))
        return "#{indent}#{receiver}.strings.remove(#{key});\n" if rhs.type == :NIL
        if string_expression?(rhs)
          # A String instance variable.
          return "#{indent}let value = (#{value rhs}).to_string();\n#{indent}#{receiver}.strings.insert(#{key}, value);\n"
        end
        unsupported(node, 'IASGN') if rule?
        "#{indent}l.set_sub(#{key}, #{sub_maker rhs})?;\n"
      when :CALL
        rule? ? rule_call(node, indent) : hook_call(node, indent)
      when :ZSUPER, :SUPER
        # `super` in `initialize`: the options are already stored.
        unsupported(node) if rule?
        ''
      when :NIL then ''
      # A literal as a statement (`rule(//) { :pop! }`) has no effect.
      when :LIT, :STR then ''
      else unsupported(node)
      end
    end

    # The value of an expression the generator can evaluate that is not a
    # String or token (collections), else nil.
    def constant_value node
      return nil unless %i[CONST COLON2 CALL VCALL].include?(node.type)

      value = evaluate(node)
      return nil if value.is_a?(String) || (value.is_a?(Class) && value <= Rouge::Token)

      value.is_a?(Enumerable) ? value : nil
    rescue Unsupported
      nil
    end

    def ivar_key name
      name.to_s.delete_prefix('@')
    end

    # `Klass.new(options)` (also `@options`, `opts` or no argument): a Rust
    # closure creating that lexer with this lexer's options.
    def sub_maker node
      unsupported(node, 'sub lexer') unless node.type == :CALL && node.children[1] == :new
      args = call_args(node.children[2])
      unsupported(node, 'sub lexer options') unless args.size <= 1 && args.all? { |a| options_node?(a) }
      klass = evaluate(node.children[0])
      unsupported(node, 'sub lexer class') unless klass.is_a?(Class) && klass < Rouge::Lexer
      # Untagged helper classes (sed's) are not in the registry.
      unsupported(node, 'untagged sub lexer') unless klass.tag && Rouge::Lexer.find(klass.tag) == klass
      @g.depend(klass.tag)
      "|l: &Lexer| l.sub_for(#{rust_str klass.tag})"
    end

    def options_node? node
      (node.type == :VCALL && node.children[0] == :options) || (node.type == :IVAR && node.children[0] == :@options) ||
        (node.type == :LVAR && node.children[0] == :opts)
    end

    # A memoized delegate `def name; @name ||= Klass.new(options); end`:
    # [ivar key, maker].
    def memo name
      meth = @lexer.instance_method(name)
      ast = RubyVM::AbstractSyntaxTree.of(meth)
      body = ast&.children&.[](2)
      body = body.children.last if body&.type == :BLOCK && body.children.size == 1
      if meth.owner == Rouge::TemplateLexer && name == :parent
        @g.depend('html')
        return ['parent', 'crate::lexers::template_lexer::parent']
      end
      unless body&.type == :OP_ASGN_OR && body.children[0].type == :IVAR && body.children[2].type == :IASGN
        unsupported(body || ast, "memo #{name}")
      end
      [ivar_key(body.children[0].children[0]), sub_maker(body.children[2].children[1])]
    rescue NameError
      unsupported(nil, "call #{name}")
    end

    # A lexer expression for `delegate`/`reset!`: [key, maker or nil].
    def sub_target node
      case node.type
      when :IVAR then [ivar_key(node.children[0]), nil]
      when :VCALL then memo(node.children[0])
      else unsupported(node, 'delegate to an instance')
      end
    end

    def hook_ops name, args, node, indent
      case name
      when :push then "#{indent}l.push_named(#{state_name args[0]})?;\n"
      when :reset_stack then "#{indent}l.reset()?;\n"
      else unsupported(node, "hook call #{name}")
      end
    end

    def hook_call node, indent
      recv, meth, args = node.children
      unsupported(node, "hook call #{meth}") unless meth == :reset! && args.nil?
      key, maker = sub_target(recv)
      "#{indent}l.reset_sub(#{rust_str key}, #{maker ? "Some(#{maker})" : 'None'})?;\n"
    end

    # `@sub.reset!` inside a rule.
    def rule_call node, indent
      recv, meth, args = node.children
      unsupported(node, "call #{meth}") unless meth == :reset! && args.nil? && %i[IVAR VCALL].include?(recv.type)
      key, maker = sub_target(recv)
      "#{indent}s.lexer.reset_sub(#{rust_str key}, #{maker ? "Some(#{maker})" : 'None'})?;\n"
    end

    def ops name, args, node, indent
      case name
      when :token
        unsupported(node, 'token arity') unless [1, 2].include?(args.size)
        val = args[1] ? value(args[1]) : 'm.text()'
        "#{indent}s.token(#{token args[0]}, #{val});\n"
      when :groups
        "#{indent}s.groups(m, &[#{args.map { |a| token a }.join(', ')}]);\n"
      when :push
        return "#{indent}s.push_current()?;\n" if args.empty?

        "#{indent}s.push(#{state_name args[0]})?;\n"
      when :pop!
        times = args.empty? ? 1 : integer(args[0])
        "#{indent}s.pop(#{times})?;\n"
      when :goto then "#{indent}s.goto(#{state_name args[0]})?;\n"
      when :reset_stack then "#{indent}s.reset_stack();\n"
      when :recurse
        val = args[0] ? value(args[0]) : 'm.text()'
        "#{indent}s.recurse(#{val})?;\n"
      when :delegate
        val = args[1] ? value(args[1]) : 'm.text()'
        if %i[IVAR VCALL].include?(args[0].type)
          key, maker = sub_target(args[0])
          return "#{indent}s.delegate_existing(#{rust_str key}, #{val})?;\n" unless maker

          return "#{indent}s.delegate_sub(#{rust_str key}, #{maker}, #{val})?;\n"
        end
        target = evaluate(args[0])
        unless target.is_a?(Class) && target < Rouge::Lexer
          unsupported(node, 'delegate to an instance')
        end
        @g.depend(target.tag)
        "#{indent}s.delegate_class(#{rust_str target.tag}, #{val})?;\n"
      else unsupported(node, "call #{name}")
      end
    end

    def state_name node
      sym = literal(node)
      unsupported(node, 'state name') unless sym.is_a?(Symbol) || sym.is_a?(String)
      @g.state_index(@states, sym)
      rust_str sym.to_s
    end

    def integer node
      val = literal(node)
      unsupported(node, 'integer') unless val.is_a?(Integer)
      val
    end

    def literal node
      case node.type
      when :LIT, :STR then node.children[0]
      else unsupported(node, 'literal')
      end
    end

    def literal_string node
      val = literal(node)
      unsupported(node, 'string literal') unless val.is_a?(String)
      rust_str val
    end

    def token node
      return local(node.children[0]) if node.type == :DVAR && @locals[node.children[0]] == :token

      @g.token_const(evaluate(node))
    end

    # Whether an assigned expression is a token type (a constant, or a
    # conditional choosing between token expressions).
    def token_expression? node
      case node.type
      when :CONST, :COLON2, :COLON3
        value = evaluate(node)
        value.is_a?(Class) && value <= Rouge::Token
      when :IF, :UNLESS then [node.children[1], node.children[2]].compact.all? { |n| token_expression?(n) }
      when :DVAR then @locals[node.children[0]] == :token
      # `push :state; Keyword`: statements, then the token.
      when :BLOCK then token_expression?(node.children.last)
      else false
      end
    rescue Unsupported
      false
    end

    def token_expr node
      case node.type
      when :IF, :UNLESS
        cond, yes, no = node.children
        unsupported(node, 'token conditional without else') unless no
        test = condition(cond)
        test = "!(#{test})" if node.type == :UNLESS
        "if #{test} { #{token_expr yes} } else { #{token_expr no} }"
      when :BLOCK
        effects = node.children[0..-2].map { |n| stmt(n, '') }.join(' ').tr("\n", ' ')
        "{ #{effects}#{token_expr node.children.last} }"
      else token(node)
      end
    end

    # A constant expression evaluated where the block runs: lexical
    # constants through the block's binding, `self.class…` on this lexer.
    def evaluate node
      case node.type
      when :CONST, :COLON2, :COLON3
        @binding.respond_to?(:binding) ? @binding.binding.eval(const_path(node)) : lexical_const(const_path(node))
      when :DVAR, :LVAR
        # A local the block closes over (e.g. a `reserved = %w(…)` list).
        name = node.children[0]
        return @consts[name] if @consts.key?(name)

        unsupported(node, 'evaluate DVAR') if @locals.key?(name) || name == @param || !@binding.respond_to?(:binding)
        @binding.binding.local_variable_get(name)
      when :CALL
        recv, meth, args = node.children
        if self_class?(recv) && args.nil?
          @lexer.public_send(meth)
        elsif meth == :[] && (key = call_args(args)).size == 1 && %i[LIT STR].include?(key[0].type)
          # `self.class.keywords[:command]`.
          evaluate(recv)[literal(key[0])]
        else
          unsupported(node, "evaluate #{meth}")
        end
      when :VCALL
        @lexer.new.send(node.children[0])
      else unsupported(node, "evaluate #{node.type}")
      end
    rescue Unsupported
      raise
    rescue StandardError, ScriptError => e
      raise Unsupported, "evaluate: #{e.message.gsub(/:0x\h+/, '')}"
    end

    # A constant from a method of `@binding` (a class): looked up like its
    # lexical scope, the class and then each enclosing module by name.
    def lexical_const path
      scopes = []
      name = @binding.name
      while name
        scopes << Object.const_get(name)
        name = name.include?('::') ? name.sub(/::[^:]+\z/, '') : nil
      end
      scopes << Object
      scope = scopes.find { |mod| mod.const_defined?(path.split('::').first) }
      raise NameError, "uninitialized constant #{path}" unless scope

      scope.const_get(path)
    end

    def self_class? node
      node.type == :CALL && node.children[0].type == :SELF && node.children[1] == :class && node.children[2].nil?
    end

    def const_path node
      case node.type
      when :CONST then node.children[0].to_s
      when :COLON2
        recv, name = node.children
        recv ? "#{const_path recv}::#{name}" : name.to_s
      when :DVAR, :LVAR
        # `base::Name::Label` on a closed-over module.
        unsupported(node, 'constant path') if @locals.key?(node.children[0]) || node.children[0] == @param
        node.children[0].to_s
      when :COLON3 then "::#{node.children[0]}"
      else unsupported(node, 'constant path')
      end
    end

    # A String-valued expression (`m[i]`, `m[i].downcase`, a local, a
    # literal).
    def value node
      return local(node.children[0]) if node.type == :DVAR && @locals[node.children[0]] == :param

      case node.type
      when :CALL
        recv, meth, args = node.children
        if meth == :[] && recv.type == :DVAR && recv.children[0] == @param
          index = integer(call_args(args)[0])
          "m.group(#{index})"
        elsif meth == :escape && recv.type == :CONST && recv.children[0] == :Regexp && call_args(args).size == 1
          "&crate::lexer::regexp_escape(#{value call_args(args)[0]})"
        elsif %i[downcase upcase].include?(meth) && args.nil?
          "&#{value recv}.to_#{meth == :downcase ? 'lowercase' : 'uppercase'}()"
        else
          unsupported(node, "value #{meth}")
        end
      when :DVAR
        unsupported(node, 'value variable') unless @locals[node.children[0]] == :string
        "#{local node.children[0]}.as_str()"
      when :STR then rust_str node.children[0]
      when :IVAR then ivar_value(node)
      else unsupported(node, "value #{node.type}")
      end
    end

    def string_expression? node
      value(node)
      true
    rescue Unsupported
      false
    end

    def ivar_value node
      receiver = rule? ? 's.lexer' : 'l'
      "&#{receiver}.string(#{rust_str ivar_key(node.children[0])})"
    end

    def present? node
      node.type == :CALL && node.children[1] == :[] && node.children[0].type == :DVAR
    end

    # `m[i]` itself (nil when the group did not participate).
    def group_node? node
      node.type == :CALL && node.children[1] == :[] && node.children[0].type == :DVAR &&
        node.children[0].children[0] == @param
    end

    def optional node
      return "m.get(#{integer call_args(node.children[2])[0]})" if group_node?(node)
      if node.type == :IVAR
        receiver = rule? ? 's.lexer' : 'l'
        return "#{receiver}.strings.get(#{rust_str ivar_key(node.children[0])}).map(String::as_str)"
      end

      "Some(#{value node})"
    end

    # `==`/`!=`: Ruby's nil equals no String, so a match group compares as
    # an Option; stack sizes and integers compare as numbers.
    def compare recv, meth, arg
      if integer_expr?(recv) || integer_expr?(arg)
        return "#{integer_expr recv} #{meth} #{integer_expr arg}"
      end
      if [recv, arg].any? { |n| group_node?(n) || n.type == :IVAR }
        return "#{optional recv} #{meth} #{optional arg}"
      end

      "#{value recv} #{meth} #{value arg}"
    end

    def integer_expr? node
      (node.type == :LIT && node.children[0].is_a?(Integer)) ||
        (node.type == :CALL && %i[size length].include?(node.children[1]) && node.children[2].nil? &&
          %i[VCALL FCALL].include?(node.children[0].type) && node.children[0].children[0] == :stack)
    end

    def integer_expr node
      unsupported(node, 'integer expression') unless integer_expr?(node)
      node.type == :LIT ? node.children[0].to_s : 's.lexer.stack.len()'
    end

    def condition node
      case node.type
      when :AND then "(#{condition node.children[0]}) && (#{condition node.children[1]})"
      when :OR then "(#{condition node.children[0]}) || (#{condition node.children[1]})"
      when :OPCALL
        recv, meth, args = node.children
        if meth == :! && args.nil?
          "!(#{condition recv})"
        elsif meth == :=== && recv.type == :LIT && recv.children[0].is_a?(Regexp)
          search(recv.children[0], call_args(args)[0])
        elsif %i[== !=].include?(meth)
          compare(recv, meth, call_args(args)[0])
        elsif %i[> < >= <=].include?(meth)
          arg = call_args(args)[0]
          "#{integer_expr recv} #{meth} #{integer_expr arg}"
        else
          unsupported(node, "operator #{meth}")
        end
      when :MATCH3
        re, val = node.children
        unsupported(node, 'match') unless re.type == :LIT
        search(re.children[0], val)
      when :MATCH2
        re, val = node.children
        search(re.children[0], val)
      when :FCALL
        name = node.children[0]
        args = call_args(node.children[1])
        case name
        when :state? then "s.state_is(#{state_name args[0]})"
        when :in_state? then "s.in_state(#{state_name args[0]})"
        else unsupported(node, "condition #{name}")
        end
      when :CALL
        recv, meth, args = node.children
        case meth
        when :include?, :member?, :key?
          collection = evaluate(recv)
          values = collection.is_a?(Hash) ? collection.keys : collection.to_a
          # A Symbol never equals the String a match yields.
          unsupported(node, 'collection') unless values.all? { |v| v.is_a?(String) }
          "contains(#{@g.set_const values}, #{value call_args(args)[0]})"
        when :==, :!=
          compare(recv, meth, call_args(args)[0])
        when :start_with?, :end_with?
          prefix = value(call_args(args)[0])
          "#{value recv}.#{meth == :start_with? ? 'starts_with' : 'ends_with'}(#{prefix})"
        when :nil?
          unsupported(node, 'condition nil?') unless group_node?(recv)
          "#{optional recv}.is_none()"
        when :empty?
          "#{value recv}.is_empty()"
        when :[]
          unsupported(node, 'condition index') unless present?(node)
          "m.get(#{integer call_args(args)[0]}).is_some()"
        else unsupported(node, "condition #{meth}")
        end
      when :ITER
        # `collection.any? { |x| condition }` over a constant collection.
        call, scope = node.children
        unsupported(node, 'condition ITER') unless call.type == :CALL && call.children[1] == :any? && call.children[2].nil?
        collection = evaluate(call.children[0])
        values = collection.is_a?(Hash) ? collection.keys : collection.to_a
        unsupported(node, 'collection') unless values.all? { |v| v.is_a?(String) }
        param = scope.children[0].first
        unsupported(node, 'iterator variable') if @locals.key?(param)
        @locals[param] = :param
        test = condition(scope.children[2])
        @locals.delete(param)
        "#{@g.set_const values}.iter().any(|#{local param}| { let #{local param}: &str = #{local param}; #{test} })"
      when :DVAR
        unsupported(node, 'condition variable')
      else unsupported(node, "condition #{node.type}")
      end
    end

    def search re, val_node
      "search(#{@g.regex re}, #{value val_node})"
    end
  end

  def self.generate tokens
    install_hook
    generator = Generator.new(tokens)
    lexers = Rouge::Lexer.all.sort_by(&:tag)
    items = lexers.map { |lexer| generator.lexer(lexer) }
    # A lexer delegating to an incomplete lexer is incomplete too.
    missing_by_tag = items.each_with_index.to_h { |(_, _, missing), i| [lexers[i].tag, missing] }
    loop do
      changed = false
      generator.deps.each do |tag, deps|
        deps.each do |dep|
          next if dep == tag || missing_by_tag.fetch(dep, []).empty?

          note = "delegates to unported #{dep}"
          next if missing_by_tag[tag].include?(note)

          missing_by_tag[tag] << note
          changed = true
        end
      end
      break unless changed
    end
    items = items.map do |ident, source, missing|
      [ident, source.sub('__MISSING__', "&[#{missing.map { |m| rust_str m }.join(', ')}]"), missing]
    end
    registry = Rouge::Lexer.send(:registry).sort_by { |name, _| name.b }
    out = +"// Generated by scripts/generate_rouge.rb from Rouge #{ROUGE_VERSION}. Do not edit by hand.\n\n"
    out << "#![allow(clippy::all, dead_code, non_snake_case, unused_assignments, unused_mut, unused_parens, unused_variables)]\n\n"
    out << "use crate::lexer::{contains, search, Action, LexError, Lexer, LexerDef, Match, Next, Rule, Scan, StateDef, Tables};\n"
    out << "use crate::regex::Regex;\n"
    out << "use crate::token as T;\n"
    out << "use std::sync::OnceLock;\n\n"
    out << "pub(crate) static REGEXES: &[(&str, u32)] = &[\n"
    generator.regexes.each { |source, options| out << "    (#{rust_str source}, #{options}),\n" }
    out << "];\n\n"
    out << "#[allow(clippy::declare_interior_mutable_const)]\n"
    out << "const UNCOMPILED: OnceLock<Regex> = OnceLock::new();\n"
    out << "static COMPILED: [OnceLock<Regex>; #{generator.regexes.size}] = [UNCOMPILED; #{generator.regexes.size}];\n\n"
    generator.sets.each { |values, name| out << "static #{name}: &[&str] = &[#{values.map { |v| rust_str v }.join(', ')}];\n" }
    out << "\n"
    generator.callbacks.each { |cb| out << cb << "\n" }
    items.each { |_, source, _| out << source << "\n" }
    out << "pub(crate) static TABLES: Tables = Tables {\n"
    out << "    regexes: REGEXES,\n"
    out << "    compiled: &COMPILED,\n"
    out << "    lexers: &[#{items.map { |ident, _, _| "&#{ident}" }.join(', ')}],\n"
    out << "    registry: &[\n"
    registry.each { |name, lexer| out << "        (#{rust_str name}, #{lexers.index(lexer)}),\n" }
    out << "    ],\n};\n"
    report = items.map { |ident, _, missing| [ident, missing] }
    [out, report]
  end
end
