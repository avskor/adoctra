'use strict'

const https = require('node:https')

module.exports.register = function () {
  this.on('beforePublish', async ({ siteCatalog }) => {
    const contents = await new Promise((resolve, reject) => {
      const buffer = []
      https
        .get('https://gitlab.com/antora/antora/-/raw/HEAD/README.adoc', (response) => {
          response.on('data', (chunk) => buffer.push(chunk.toString()))
          response.on('end', () => resolve(buffer.join('').trimEnd()))
        })
        .on('error', reject)
    })
    siteCatalog.addFile({ contents: Buffer.from(contents), out: { path: 'README.adoc' } })
  })
}
