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

  # A Ruby file under Rouge's lexers directory as a Rust module name.
  def self.module_name path
    rel = path.delete_prefix("#{LEXERS_DIR}/").delete_suffix('.rb')
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

  class Generator
    attr_reader :regexes, :sets, :callbacks

    def initialize tokens
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
    def lexer lexer
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
      source << "    missing: &[#{missing.map { |m| rust_str m }.join(', ')}],\n"
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
          body = Translator.new(self, lexer, blk, states).translate
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
        if RougeLexers.hand?(path, "start_#{line}")
          "crate::lexers::#{RougeLexers.module_name path}::start_#{line}"
        else
          missing << "#{where} (start)"
          'crate::lexer::unported_hook'
        end
      end
    end

    def owner_file lexer, meth
      lexer.instance_method(meth).source_location&.first
    end

    def hook_init lexer, missing
      owner = lexer.instance_method(:initialize).owner
      return 'None' if [Rouge::Lexer, Rouge::RegexLexer].include?(owner) || !(owner < Rouge::Lexer)

      path = owner_file(lexer, :initialize)
      return "Some(crate::lexers::#{RougeLexers.module_name path}::init)" if RougeLexers.hand?(path, 'init')

      missing << "#{RougeLexers.module_name path}.rs (initialize)"
      'None'
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
      return "Some(crate::lexers::#{RougeLexers.module_name path}::stream)" if RougeLexers.hand?(path, 'stream')

      missing << "#{RougeLexers.module_name path}.rs (stream_tokens)"
      'None'
    end
  end

  # Translates the AST of a rule block into the body of a Rust callback.
  class Translator
    def initialize generator, lexer, blk, states
      @g = generator
      @lexer = lexer
      @blk = blk
      @states = states
      @ast = RubyVM::AbstractSyntaxTree.of(blk)
      raise Unsupported, 'no AST' unless @ast

      @param = @ast.children[0].first
      args = @ast.children[1]
      raise Unsupported, 'block arity' if args && args.children[0] > 1
    end

    def translate
      stmts(@ast.children[2], '    ')
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

    def stmt node, indent
      case node.type
      when :FCALL, :VCALL
        name = node.children[0]
        args = node.type == :FCALL ? call_args(node.children[1]) : []
        ops(name, args, node, indent)
      when :IF, :UNLESS
        cond, yes, no = node.children
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
      when :NIL then ''
      else unsupported(node)
      end
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
        target = evaluate(args[0])
        unless target.is_a?(Class) && target < Rouge::Lexer
          unsupported(node, 'delegate to an instance')
        end
        val = args[1] ? value(args[1]) : 'm.text()'
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
      @g.token_const(evaluate(node))
    end

    # A constant expression evaluated where the block runs: lexical
    # constants through the block's binding, `self.class…` on this lexer.
    def evaluate node
      case node.type
      when :CONST, :COLON2, :COLON3 then @blk.binding.eval(const_path(node))
      when :CALL
        recv, meth, args = node.children
        if self_class?(recv) && args.nil?
          @lexer.public_send(meth)
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
      raise Unsupported, "evaluate: #{e.message}"
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
      when :COLON3 then "::#{node.children[0]}"
      else unsupported(node, 'constant path')
      end
    end

    # A String-valued expression (`m[i]`, `m[i].downcase`, a literal).
    def value node
      case node.type
      when :CALL
        recv, meth, args = node.children
        if meth == :[] && recv.type == :DVAR && recv.children[0] == @param
          index = integer(call_args(args)[0])
          "m.group(#{index})"
        elsif %i[downcase upcase].include?(meth) && args.nil?
          "&#{value recv}.to_#{meth == :downcase ? 'lowercase' : 'uppercase'}()"
        else
          unsupported(node, "value #{meth}")
        end
      when :STR then rust_str node.children[0]
      else unsupported(node, "value #{node.type}")
      end
    end

    def present? node
      node.type == :CALL && node.children[1] == :[] && node.children[0].type == :DVAR
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
          test = "#{value recv} #{meth} #{value call_args(args)[0]}"
          test
        when :start_with?, :end_with?
          prefix = literal_string(call_args(args)[0])
          "#{value recv}.#{meth == :start_with? ? 'starts_with' : 'ends_with'}(#{prefix})"
        when :empty?
          "#{value recv}.is_empty()"
        when :[]
          unsupported(node, 'condition index') unless present?(node)
          "m.get(#{integer call_args(args)[0]}).is_some()"
        else unsupported(node, "condition #{meth}")
        end
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
    registry = Rouge::Lexer.send(:registry).sort_by { |name, _| name.b }
    out = +"// Generated by scripts/generate_rouge.rb from Rouge #{ROUGE_VERSION}. Do not edit by hand.\n\n"
    out << "#![allow(clippy::all, dead_code, unused_parens, unused_variables)]\n\n"
    out << "use crate::lexer::{contains, search, Action, LexError, LexerDef, Match, Next, Rule, Scan, StateDef, Tables};\n"
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
