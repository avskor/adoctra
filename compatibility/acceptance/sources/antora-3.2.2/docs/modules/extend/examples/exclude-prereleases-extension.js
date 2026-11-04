'use strict'

module.exports.register = function () {
  this.once('contentAggregated', ({ contentAggregate }) => {
    this.updateVariables({
      contentAggregate: contentAggregate.filter((componentVersionBucket) => !componentVersionBucket.prerelease),
    })
  })
}
