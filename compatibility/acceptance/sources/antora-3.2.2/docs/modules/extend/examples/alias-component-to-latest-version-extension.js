'use strict'

module.exports.register = function () {
  this.once('contentClassified', ({ contentCatalog }) => {
    contentCatalog.getComponents().forEach((component) => {
      if (component.versions.find((it) => !it.version)) return
      const rel = contentCatalog.resolvePage('index.adoc', { component: component.name })
      if (!rel) return
      contentCatalog.addFile({
        src: { component: component.name, version: '', module: 'ROOT', family: 'alias', relative: 'index.adoc' },
        rel,
      })
    })
  })
}
