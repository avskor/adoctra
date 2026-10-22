# frozen_string_literal: true

# Read the parser tree without invoking a converter or lazy inline substitutions.
# Shared schema and the adoctra adapter live in structure.py.
def structure_document(document)
  { context: 'document', has_title: document.header?,
    blocks: document.blocks.map { |block| structure_block block } }
end

def structure_item(item)
  { checkbox: item.attr?('checkbox') ? item.attr?('checked') : nil,
    blocks: item.blocks.map { |block| structure_block block } }
end

def structure_block(block)
  context = block.context
  result = { context: context.to_s, id: block.id, roles: block.roles }
  result[:level] = block.level if [:section, :floating_title].include? context
  if [:preamble, :open, :example, :sidebar, :quote, :admonition, :section].include? context
    result[:blocks] = block.blocks.map { |child| structure_block child }
    result[:content_model] = block.content_model.to_s unless [:preamble, :section].include? context
  end
  result[:admonition] = block.attr('name') if context == :admonition
  if [:ulist, :olist, :colist].include? context
    result[:items] = block.items.map { |item| structure_item item }
  elsif context == :dlist
    result[:items] = block.items.map do |terms, description|
      { terms: terms.size, description: description && structure_item(description) }
    end
  elsif context == :table
    result[:columns] = block.columns.size
    [:head, :body, :foot].each do |group|
      result[group] = block.rows.public_send(group).map do |row|
        row.map do |cell|
          { style: cell.style&.to_s, colspan: cell.colspan || 1, rowspan: cell.rowspan || 1,
            document: cell.inner_document && structure_document(cell.inner_document) }
        end
      end
    end
  end
  result
end
