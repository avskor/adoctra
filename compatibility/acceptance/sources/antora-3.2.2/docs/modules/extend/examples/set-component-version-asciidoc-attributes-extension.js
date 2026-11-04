'use strict'

module.exports.register = function () {
  this.once('contentClassified', ({ contentCatalog, siteAsciiDocConfig }) => {
    for (const component of contentCatalog.getComponents()) {
      for (const componentVersion of component.versions) {
        if (!componentVersion.prerelease) continue
        if (!componentVersion.asciidoc || componentVersion.asciidoc === siteAsciiDocConfig) {
          componentVersion.asciidoc = Object.assign({}, siteAsciiDocConfig)
          componentVersion.asciidoc.attributes = Object.assign({}, siteAsciiDocConfig.attributes)
        }
        componentVersion.asciidoc.attributes['page-component-version-is-prerelease'] = ''
      }
    }
  })
}
