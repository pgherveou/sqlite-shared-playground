import init, { BrowserClient, ping } from './pkg/sqlite_shared_playground.js';

await init();
let database;

self.onmessage = async ({ data: { id, method, argument } }) => {
  try {
    let result;
    if (method === 'ping') {
      result = ping();
    } else if (method === 'open') {
      database = await BrowserClient.open(argument);
    } else {
      if (!database) throw new Error('Database is not open');
      switch (method) {
        case 'create': result = await database.create_purse(argument.name, argument.tag); break;
        case 'get': result = await database.purse(argument); break;
        case 'list': result = await database.purses_tagged(argument); break;
        case 'batch': result = await database.create_purses(argument.names, argument.tag); break;
        case 'sql_failure': result = await database.sql_failure(); break;
        case 'unknown': result = await database.unknown_operation(); break;
        case 'held': result = await database.held_query(); break;
        case 'close': result = await database.close(); break;
        case 'terminate': result = database.terminate(); break;
        default: throw new Error(`Unknown caller method: ${method}`);
      }
    }
    self.postMessage({ id, result });
  } catch (error) {
    self.postMessage({ id, error: String(error) });
  }
};
self.postMessage({ ready: true });
