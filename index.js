

const { watch, getInfo } = require('./fsevents-rs.js');

let stop = watch(__dirname, (path, flags) => {
  const info = getInfo(path, flags);
  console.log(info);
});

setTimeout(() => {
  console.log('Stopping');
  stop();
}, 10000);