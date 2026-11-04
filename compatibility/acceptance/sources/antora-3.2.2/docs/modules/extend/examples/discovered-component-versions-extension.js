'use strict'

module.exports.register = function () {
  this.once('contentAggregated', ({ contentAggregate }) => {
    console.log('Discovered the following component versions')
    contentAggregate.forEach((bucket) => {
      const sources = bucket.origins.map(({ url, refname }) => ({ url, refname }))
      console.log({ name: bucket.name, version: bucket.version, files: bucket.files.length, sources })
    })
  })
}
