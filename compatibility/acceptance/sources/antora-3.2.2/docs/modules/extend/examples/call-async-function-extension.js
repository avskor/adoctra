'use strict'

module.exports.register = function () {
  this.on('contentClassified', async () => {
    const logger = this.getLogger('my-extension')
    await doWork(logger, 'task completion message') // <.>
    logger.info('Event listener finished')
  })
}

function doWork (logger, message) {
  return new Promise((resolve) =>
    setTimeout(() => {
      // do some async work...
      logger.info(message)
      resolve()
    }, 500)
  )
}
