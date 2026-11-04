'use strict'

module.exports.register = function ({ config: { dataUrl, component = 'ROOT', version = '' } }) {
  this.once('contentClassified', async ({ contentCatalog }) => {
    const notes = await fetchReleaseNotes(dataUrl)
    const adoc = ['= Release Notes', '']
    notes.forEach((note) => {
      const projectPath = note.html_url.match(/https:\/\/github.com\/([^/]+\/[^/]+)/)[1]
      adoc.push(`* *${projectPath}* (${note.name}) [${new Date(Date.parse(note.published_at)).toLocaleString()}]`)
    })
    contentCatalog.addFile({
      path: 'release-notes.adoc',
      contents: Buffer.from(adoc.join('\n') + '\n'),
      src: { component, version, module: 'ROOT', family: 'page', relative: 'release-notes.adoc' },
    })
  })
}

async function fetchReleaseNotes (url) {
  return fetch(url).then((response) => response.json())
}
