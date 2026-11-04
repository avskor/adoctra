'use strict'

module.exports.register = function () {
  const relativize = this.require('@antora/asciidoc-loader/util/compute-relative-url-path')
  this.once('documentsConverted', ({ contentCatalog }) => {
    contentCatalog.getComponents().forEach(({ versions }) => {
      versions.forEach(({ name: component, version, url }) => {
        const pageList = ['<ul>']
        const pages = contentCatalog
          .findBy({ component, version, family: 'page' })
          .sort((a, b) => a.title.localeCompare(b.title))
        for (const page of pages) {
          pageList.push(`<li><a href="${relativize(url, page.pub.url)}">${page.title}</a></li>`)
        }
        pageList.push('</ul>')
        const pageListFile = contentCatalog.addFile({
          contents: Buffer.from(pageList.join('\n') + '\n'),
          src: { component, version, module: 'ROOT', family: 'page', relative: 'all-pages.html' },
        })
        pageListFile.asciidoc = { doctitle: 'All Pages' }
        // use the following assignment instead to use a separate layout (e.g., report.hbs)
        //pageListFile.asciidoc = { doctitle: 'All Pages', attributes: { 'page-layout': 'report' } }
      })
    })
  })
}
