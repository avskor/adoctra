'use strict'

module.exports.register = function () {
  this.on('documentsConverted', ({ contentCatalog }) => {
    contentCatalog.getPages((page) => {
      if (page.out && page.asciidoc?.attributes['page-unpublish'] != null) delete page.out
    })
  })
}
