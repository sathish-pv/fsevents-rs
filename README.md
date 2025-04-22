# fsevents-rs

Rust port of [fsevents](https://github.com/fsevents/fsevents) node package

Native access to MacOS FSEvents in [Node.js](https://nodejs.org/)


```js
const fsevents = require('fsevents-rs');

// To start observation
const stop = fsevents.watch(__dirname, (path, flags, id) => {
  const info = fsevents.getInfo(path, flags);
});

// To end observation
stop();
```

> **Important note:** The API behaviour is slightly different from typical JS APIs. The `stop` function **must** be
> retrieved and stored somewhere, even if you don't plan to stop the watcher. If you forget it, the garbage collector
> will eventually kick in, the watcher will be unregistered, and your callbacks won't be called anymore.

The callback passed as the second parameter to `.watch` get's called whenever the operating system detects a
a change in the file system. It takes three arguments:

###### `fsevents.watch(dirname: string, (path: string, flags: number, id: string) => void): () => Promise<undefined>`

 * `path: string` - the item in the filesystem that have been changed
 * `flags: number` - a numeric value describing what the change was
 * `id: string` - an unique-id identifying this specific event

 Returns closer callback which when called returns a Promise resolving when the watcher process has been shut down.

###### `fsevents.getInfo(path: string, flags: number, id: string): FsEventInfo`

The `getInfo` function takes the `path`, `flags` and `id` arguments and converts those parameters into a structure
that is easier to digest to determine what the change was.

The `FsEventsInfo` has the following shape:

```js
/**
 * @typedef {'created'|'modified'|'deleted'|'moved'|'root-changed'|'cloned'|'unknown'} FsEventsEvent
 * @typedef {'file'|'directory'|'symlink'} FsEventsType
 */
{
  "event": "created", // {FsEventsEvent}
  "path": "file.txt",
  "type": "file",    // {FsEventsType}
  "changes": {
    "inode": true,   // Had iNode Meta-Information changed
    "finder": false, // Had Finder Meta-Data changed
    "access": false, // Had access permissions changed
    "xattrs": false  // Had xAttributes changed
  },
  "flags": 0x100000000
}
```