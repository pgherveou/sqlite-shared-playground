import init, { serve_database } from './pkg/sqlite_shared_playground.js';

const queued = [];
self.onmessage = event => queued.push(event);
try {
  await init();
  const name = new URL(self.location.href).searchParams.get('name') ?? 'purses.sqlite3';
  const serving = serve_database(name);
  for (const event of queued) self.dispatchEvent(event);
  queued.length = 0;
  await serving;
} catch (error) {
  setTimeout(() => { throw new Error(String(error)); });
}
