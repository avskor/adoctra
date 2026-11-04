'use strict'

module.exports.register = function () {
  this.once('contentClassified', ({ siteAsciiDocConfig, contentCatalog }) => {
    console.log('site-wide attributes (compiled)')
    console.log(siteAsciiDocConfig.attributes)
    contentCatalog.getComponents().forEach((component) => {
      component.versions.forEach((componentVersion) => {
        console.log(`${componentVersion.version}@${componentVersion.name} attributes (compiled)`)
        if (componentVersion.asciidoc === siteAsciiDocConfig) {
          console.log('same as site-wide attributes')
        } else {
          console.log(componentVersion.asciidoc.attributes)
        }
      })
    })
  })
}
