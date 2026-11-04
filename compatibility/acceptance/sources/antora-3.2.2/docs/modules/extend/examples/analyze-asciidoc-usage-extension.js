'use strict'

module.exports.register = function () {
  this.once('contentClassified', ({ contentCatalog }) => {
    contentCatalog.getFiles().forEach((it) => {
      if (!it.out && it.src.mediaType === 'text/asciidoc' && it.src.family !== 'nav') {
        Object.defineProperty(it, 'contents', {
          configurable: true,
          enumerable: true,
          get: markReadAndGetContents.bind(it, it.contents),
        })
      }
    })
  })

  this.once('sitePublished', ({ contentCatalog }) => {
    const internalNavUrls = new Set()
    contentCatalog.getComponents().forEach(({ versions }) => {
      versions.forEach(({ navigation, url: defaultUrl }) => {
        internalNavUrls.add(defaultUrl)
        getInternalUrls(navigation).forEach((url) => internalNavUrls.add(url))
      })
    })
    const asciidocFiles = { used: [], unused: [] }
    contentCatalog.getFiles().forEach((it) => {
      if (it.src.mediaType !== 'text/asciidoc' || ['alias', 'nav'].includes(it.src.family)) return
      it.out
        ? asciidocFiles[internalNavUrls.has(it.pub.url) ? 'used' : 'unused'].push(it)
        : asciidocFiles[it.read ? 'used' : 'unused'].push(it)
    })
    for (const status of ['used', 'unused']) {
      console.log(status.charAt().toUpperCase() + status.slice(1) + ' AsciiDoc files:')
      for (const file of asciidocFiles[status]) console.log(generateQualifiedResourceRef(file.src))
    }
  })
}

function markReadAndGetContents (value) {
  this.read = true
  Object.defineProperty(this, 'contents', { value, writable: true })
  return value
}

function getInternalUrls (items = [], accum = []) {
  items.forEach((item) => {
    if (item.urlType === 'internal') accum.push(item.url)
    getInternalUrls(item.items, accum)
  })
  return accum
}

function generateQualifiedResourceRef ({ component, version, module: module_, family, relative }) {
  return `${version}${version ? '@' : ''}${component}:${module_}:${family}$${relative}`
}
