'use strict'

module.exports.register = function () {
  this.on('playbookBuilt', () => {
    console.time('generation time')
  })
  this.on('sitePublished', () => {
    console.timeEnd('generation time')
  })
}
