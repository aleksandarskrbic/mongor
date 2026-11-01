// Run by mongosh with `db` already connected to the target environment's
// database. Scripts are plain mongosh JavaScript, so anything you can type in
// the shell works here.
//
// Do not use `await`, and do not wrap the body in an async IIFE: mongosh parses
// a --file script as a script and not a module, where top-level `await` is a
// SyntaxError, and a floating promise can be cut off when the shell exits.
// Shell API calls are awaited for you.

db.example.createIndex({ field: 1 });
