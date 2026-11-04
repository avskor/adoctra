'use strict'

module.exports.register = function () {
  this.once('contentClassified', ({ playbook, contentCatalog }) => {
    console.log('site-wide attributes (as defined in playbook)')
    console.log(playbook.asciidoc.attributes)
    contentCatalog.getComponents().forEach((component) => {
      component.versions.forEach((componentVersion) => {
        getUniqueOrigins(contentCatalog, componentVersion).forEach((origin) => {
          console.log(`${componentVersion.version}@${componentVersion.name} attributes (as defined in antora.yml)`)
          console.log(origin.descriptor.asciidoc?.attributes || {})
        })
      })
    })
  })
}

function getUniqueOrigins (contentCatalog, componentVersion) {
  return contentCatalog
    .findBy({ component: componentVersion.name, version: componentVersion.version })
    .reduce((origins, file) => {
      const origin = file.src.origin
      if (origin && !origins.includes(origin)) origins.push(origin)
      return origins
    }, [])
}
