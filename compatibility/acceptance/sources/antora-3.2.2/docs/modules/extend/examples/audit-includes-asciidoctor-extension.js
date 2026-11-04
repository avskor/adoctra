'use strict'

module.exports.register = (registry, context) => {
  registry.$groups().$store('audit-includes', toProc(createExtensionGroup(context)))
  return registry
}

function createExtensionGroup ({ contentCatalog, file }) {
  return function () {
    this.includeProcessor(function () {
      this.prefer()
      this.process((doc, reader, target, attrs) => {
        const cursor = reader.$cursor_at_prev_line()
        const from = cursor.file?.src || file.src
        this.logger ??= require('@antora/logger')('asciidoctor')
        const resource = contentCatalog.resolveResource(target, from)
        this.logger.info({ file: resource.src, stack: [{ file: from, line: cursor.lineno }] }, `include: ${target}`)
        const delegate = doc
          .getExtensions()
          .getIncludeProcessors()
          .find((it) => it.instance !== this)
        return delegate.process_method['$[]'](doc, reader, target, global.Opal.hash(attrs))
      })
    })
  }
}

function toProc (fn) {
  return Object.defineProperty(fn, '$$arity', { value: fn.length })
}
