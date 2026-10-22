# frozen_string_literal: true

# An independent semantic converter. It never calls the HTML converter. Inline
# substitutions return paired boundary handles, preserving nesting while
# leaving content available to later substitution stages in Ruby's string API.
# The Python adapter decodes the lexical entities once and validates the schema.
class SemanticConverter < Asciidoctor::Converter::Base
  attr_accessor :attribute_names

  def initialize backend, opts = {}
    super
    @nodes = []
    @attribute_names = []
  end

  def handle event
    index = @nodes.size
    @nodes << event
    "<\uE000#{index}\uE001>"
  end

  def store node
    handle(event: :leaf, node: node)
  end

  def wrap node, text
    # Keep content exposed to the remaining substitution stages. An opaque
    # handle for the entire span would incorrectly protect {attributes} and
    # macros inside formatting, since Ruby performs quotes before those stages.
    opening = @nodes.size
    handle(event: :open, node: node) + text.to_s + handle(event: :close, opening: opening)
  end

  def inlines text
    result = []
    stack = [[nil, result]]
    text.to_s.split(/(<\uE000\d+\uE001>)/).each do |part|
      next if part.empty?
      if part.start_with? "<\uE000"
        index = part[2...-2].to_i
        event = @nodes.fetch(index)
        case event[:event]
        when :open
          node = event[:node].merge(inlines: [])
          stack[-1][1] << node
          stack << [index, node[:inlines]]
        when :close
          raise 'unbalanced semantic handles' unless stack.size > 1 && stack[-1][0] == event[:opening]
          stack.pop
        when :leaf
          stack[-1][1] << event[:node]
        end
      else
        raise 'unrecognized semantic handle' if part.include?("\uE000") || part.include?("\uE001")
        stack[-1][1] << { kind: 'text', value: part }
      end
    end
    raise 'unclosed semantic handle' unless stack.size == 1
    result
  end

  def snapshot document
    @attribute_names.to_h do |name|
      [name, document.attributes.key?(name) ? inlines(document.attributes[name].to_s) : nil]
    end
  end

  def metadata node, context = node.node_name
    { context: context, id: node.id, roles: node.roles,
      title: node.title? ? inlines(node.title) : nil }
  end

  def item node
    { inlines: inlines(node.text), checkbox: (node.attr? 'checkbox') ? (node.attr? 'checked') : nil,
      blocks: node.blocks.map { |block| block.convert } }
  end

  def image_fields node
    allowed = [:attribute_entries, *%w[alt default-alt width height title link float id role imagesdir target]]
    raise 'image options are outside inline-semantics-v1' unless node.attributes.keys.all? { |key| Integer === key || allowed.include?(key) }
    attrs = node.attributes.select { |key, _| %w[width height title link float].include? key }
    target = node.respond_to?(:target) ? node.target : (node.attr 'target')
    { target: target, src: node.image_uri(target), alt: inlines(node.attr 'alt'), attributes: attrs }
  end

  def link_metadata node
    allowed = [1, *%w[role id title window nofollow-option noopener-option]]
    raise 'link metadata is outside inline-semantics-v1' unless (node.attributes.keys - allowed).empty?
    result = {}
    result[:id] = node.id if node.id
    result[:role] = node.role if node.role && node.role != 'bare'
    %w[title window].each { |key| result[key.to_sym] = node.attr(key) if node.attr? key }
    rel = (node.option? 'nofollow') ? 'nofollow' : nil
    if (window = node.attr 'window') && (window == '_blank' || (node.option? 'noopener'))
      rel = rel ? "#{rel} noopener" : 'noopener'
    end
    result[:rel] = rel if rel
    result
  end

  def convert node, transform = node.node_name, _opts = nil
    case transform
    when 'embedded', 'document'
      header = snapshot node
      title = node.header? ? inlines(node.doctitle) : nil
      blocks = node.blocks.map { |block| block.convert }
      { attributes: { header: header, final: snapshot(node) }, title: title, blocks: blocks }
    when 'paragraph'
      { context: 'paragraph', id: node.id, roles: node.roles,
        title: node.title? ? inlines(node.title) : nil, inlines: inlines(node.content) }
    when 'section'
      { context: 'section', id: node.id, roles: node.roles, level: node.level,
        title: inlines(node.title), blocks: node.blocks.map { |block| block.convert } }
    when 'preamble', 'open', 'sidebar', 'example', 'admonition'
      result = metadata node
      result[:type] = node.attr('name') if transform == 'admonition'
      if node.content_model == :simple
        result[:inlines] = inlines(node.content)
      else
        raise 'unsupported container content model' unless node.content_model == :compound
        result[:blocks] = node.blocks.map { |block| block.convert }
      end
      result
    when 'listing', 'literal'
      metadata(node).merge(language: node.attr('language'), inlines: inlines(node.content))
    when 'ulist', 'olist'
      metadata(node).merge(style: node.style, items: node.items.map { |entry| item entry })
    when 'dlist'
      metadata(node).merge(items: node.items.map { |terms, description|
        { terms: terms.map { |term| inlines(term.text) }, description: description ? item(description) : nil }
      })
    when 'image'
      metadata(node).merge(image_fields(node))
    when 'inline_quoted'
      kind = { strong: 'strong', emphasis: 'emphasis', monospaced: 'monospace',
               mark: 'mark', superscript: 'superscript', subscript: 'subscript', unquoted: 'span' }.fetch(node.type)
      wrap({ kind: kind, id: node.id, roles: node.roles }, node.text)
    when 'inline_anchor'
      case node.type
      when :link
        result = { kind: 'link', target: node.target }
        fields = link_metadata node
        result[:metadata] = fields unless fields.empty?
        wrap(result, node.text)
      when :ref
        store(kind: 'anchor', id: node.id)
      when :xref
        raise 'implicit xref label is outside inline-semantics-v1' unless node.text
        raise 'xref metadata is outside inline-semantics-v1' unless (node.attributes.keys - ['path', 'fragment', 'refid']).empty?
        wrap({ kind: 'xref', target: node.target }, node.text)
      else
        raise "unsupported semantic anchor: #{node.type}"
      end
    when 'inline_break'
      # Ruby supplies the preceding text as part of this inline callback.
      node.text.to_s + store(kind: 'line-break')
    when 'inline_button'
      wrap({ kind: 'button' }, node.text)
    when 'inline_kbd'
      store(kind: 'kbd', keys: (node.attr 'keys').map { |key| inlines key })
    when 'inline_menu'
      store(kind: 'menu', menu: inlines(node.attr 'menu'),
            submenus: (node.attr 'submenus').map { |label| inlines label },
            item: (node.attr 'menuitem') ? inlines(node.attr 'menuitem') : nil)
    when 'inline_image'
      raise 'icons are outside inline-semantics-v1' if node.type == 'icon'
      store({ kind: 'image', id: node.id || (node.attr 'id'), roles: node.roles }.merge(image_fields(node)))
    else
      raise "unsupported inline-semantics-v1 node: #{transform}"
    end
  end
end
