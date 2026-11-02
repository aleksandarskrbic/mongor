// Write whatever puts the database back the way it was:
//
//   db.users.dropIndex("email_1");
//
// Plenty of migrations can't be undone, and that's fine - leave this file as it
// is. mongor will tell you there's no rollback rather than run an empty script
// and claim it undid something.
