'use strict'

module.exports.register = function () {
  // NOTE you could replace buildNavigation step with a noop so it doesn't try to compute the navigation
  this.once('navigationBuilt', ({ contentCatalog }) => {
    const { buildAlternateNavigation } = this.getFunctions()
    contentCatalog.getComponents().forEach((component) => {
      component.versions.forEach((componentVersion) => {
        const navContents = []
        const pagesByModule = componentVersion.files.reduce((accum, file) => {
          if (!(file.src.family === 'page' && file.out)) return accum
          const moduleName = file.src.module
          if (!(moduleName in accum)) accum[moduleName] = []
          accum[moduleName].push(file)
          return accum
        }, {})
        for (const moduleName in pagesByModule) {
          pagesByModule[moduleName].sort((a, b) => a.asciidoc.navtitle.localeCompare(b.asciidoc.navtitle))
        }
        if ('ROOT' in pagesByModule) {
          const indexPage = contentCatalog.getById({
            component: component.name,
            version: componentVersion.version,
            module: 'ROOT',
            family: 'page',
            relative: 'index.adoc',
          })
          pagesByModule.ROOT.forEach((page) => {
            if (!(indexPage && page === indexPage)) navContents.push(`* xref:${page.src.relative}[]`)
          })
          delete pagesByModule.ROOT
        }
        Object.keys(pagesByModule)
          .sort()
          .forEach((moduleName) => {
            const indexPage = contentCatalog.getById({
              component: component.name,
              version: componentVersion.version,
              module: moduleName,
              family: 'page',
              relative: 'index.adoc',
            })
            indexPage
              ? navContents.push(`* xref:${moduleName}:index.adoc[]`)
              : navContents.push(`* ${moduleName.charAt().toUpperCase()}${moduleName.substring(1)}`)
            pagesByModule[moduleName].forEach((page) => {
              if (!(indexPage && page === indexPage)) navContents.push(` ** xref:${moduleName}:${page.src.relative}[]`)
            })
          })
        const navFile = contentCatalog.addFile({
          contents: Buffer.from(navContents.join('\n') + '\n'),
          path: 'modules/ROOT/auto-nav.adoc',
          src: {
            component: component.name,
            version: componentVersion.version,
            family: 'nav',
            module: 'ROOT',
            relative: 'auto-nav.adoc',
          },
        })
        contentCatalog.removeFile(navFile)
        componentVersion.navigation = buildAlternateNavigation(contentCatalog, componentVersion, [navFile])
      })
    })
  })
}
