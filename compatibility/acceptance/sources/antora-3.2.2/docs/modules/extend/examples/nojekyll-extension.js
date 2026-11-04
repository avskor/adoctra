'use strict'

module.exports.register = function () {
  this.on('beforePublish', ({ siteCatalog }) => {
    siteCatalog.addFile({ contents: Buffer.alloc(0), out: { path: '.nojekyll' } })
  })
}
