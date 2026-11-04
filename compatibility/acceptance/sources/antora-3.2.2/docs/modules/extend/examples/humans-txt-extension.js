'use strict'

module.exports.register = function ({ config }) {
  this.on('beforePublish', ({ siteCatalog }) => {
    const teamInfo = '/* TEAM */\n' + config.names.map((name) => `Name: ${name}`).join('\n')
    const contents = Buffer.from(teamInfo + '\n')
    siteCatalog.addFile({ contents, out: { path: 'humans.txt' } })
  })
}
