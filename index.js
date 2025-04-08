

const { globals, flags, constants, start } = require('./index.node')
console.log(globals, flags, constants);

let instance = start('asd',() => {});
console.log(instance);

