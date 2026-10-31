# frozen_string_literal: true

# The reference CLI for scripts/cli_matrix.py: Ruby's own `asciidoctor`
# command (Asciidoctor::Cli::Invoker, like bin/asciidoctor) with the pinned
# gem, the pinned Rouge and no other optional highlighter gems (as the
# golden references, the upstream audit and CI).
# Usage: ruby scripts/ruby-cli.rb [asciidoctor arguments]...
gem 'rouge', File.read(File.expand_path('../compatibility/rouge-version', __dir__)).strip

module UnavailableHighlighters
  NAMES = %w(coderay pygments pygments.rb).freeze

  def require name
    if NAMES.include?(feature = name.to_s.split('/').first)
      error = LoadError.new "cannot load such file -- #{name}"
      error.define_singleton_method(:path) { feature }
      raise error
    end
    super
  end
end
Kernel.prepend UnavailableHighlighters
Object.prepend UnavailableHighlighters

gem 'asciidoctor', File.read(File.expand_path('../compatibility/asciidoctor-version', __dir__)).strip
require 'asciidoctor'
require 'asciidoctor/cli'

invoker = Asciidoctor::Cli::Invoker.new ARGV
GC.start
invoker.invoke!
exit invoker.code
