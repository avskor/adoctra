'use strict'

module.exports.register = function () {
  this.once('contentClassified', ({ contentCatalog }) => {
    for (const file of getAssetFiles(contentCatalog)) {
      file.pub = new Proxy(file.pub, {
        get (target, property) {
          if (property === 'url') target.referenced = true
          return target[property]
        },
      })
    }
  })

  this.once('beforePublish', ({ contentCatalog }) => {
    for (const file of getAssetFiles(contentCatalog)) {
      if (!file.pub.referenced) delete file.out
    }
  })
}

function getAssetFiles (contentCatalog) {
  return contentCatalog.getFiles(
    (candidate) =>
      contentCatalog.publishableFamilies.has(candidate.src.family) && candidate.pub && candidate.src.family !== 'page'
  )
}
