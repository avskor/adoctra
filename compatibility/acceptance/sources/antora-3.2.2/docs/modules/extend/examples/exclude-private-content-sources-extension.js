'use strict'

module.exports.register = function () {
  this.on('playbookBuilt', function ({ playbook }) {
    playbook.content.sources = playbook.content.sources.filter(({ url }) => !url.startsWith('git@'))
    this.updateVariables({ playbook })
  })
}
