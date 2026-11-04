'use strict'

module.exports.register = function () {
  this.on('beforeProcess', ({ siteAsciiDocConfig }) => {
    const buildDate = new Date().toISOString()
    siteAsciiDocConfig.attributes['build-date'] = buildDate
  })
}
