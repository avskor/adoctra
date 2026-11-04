'use strict'

module.exports.register = function () {
  this.once('contentAggregated', ({ contentAggregate }) => {
    const targetWebUrl = 'https://gitlab.com/antora/antora'
    for (const bucket of contentAggregate) {
      if (!bucket.origins.some((it) => it.webUrl === targetWebUrl)) continue
      for (const { src } of bucket.files) {
        if (!(src.editUrl && src.origin.webUrl === targetWebUrl)) continue
        let editUrlPattern = src.origin.editUrlPattern
        const pathIdx = editUrlPattern.indexOf('/', 8)
        editUrlPattern = `${editUrlPattern.substring(0, pathIdx)}/-/ide/project${editUrlPattern.substring(pathIdx)}`
        src.editUrl = editUrlPattern.replace('%s', src.path)
      }
    }
  })
}
