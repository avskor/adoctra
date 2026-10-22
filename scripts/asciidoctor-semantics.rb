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
    when 'preamble', 'open'
      raise 'simple open is outside inline-semantics-v1' if node.content_model != :compound
      { context: transform, id: node.id, roles: node.roles,
        title: node.title? ? inlines(node.title) : nil,
        blocks: node.blocks.map { |block| block.convert } }
    when 'inline_quoted'
      kind = { strong: 'strong', emphasis: 'emphasis', monospaced: 'monospace',
               mark: 'mark', superscript: 'superscript', subscript: 'subscript', unquoted: 'span' }.fetch(node.type)
      wrap({ kind: kind, id: node.id, roles: node.roles }, node.text)
    when 'inline_anchor'
      case node.type
      when :link
        # Link target and label are already resolved by the Ruby substitutor.
        # Full link metadata and implicit xrefs require a later schema version.
        raise 'link metadata is outside inline-semantics-v1' if node.id || (node.role && node.role != 'bare') || !(node.attributes.keys - ['role']).empty?
        wrap({ kind: 'link', target: node.target }, node.text)
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
    else
      raise "unsupported inline-semantics-v1 node: #{transform}"
    end
  end
end
