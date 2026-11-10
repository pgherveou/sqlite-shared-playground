import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { createServer } from './server.mjs';

let gate;
function expectGate() {
  assert.equal(gate, undefined);
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('SQLite did not reach its gate')), 10000);
    gate = response => {
      clearTimeout(timeout);
      resolve(() => { response.end('release'); gate = undefined; });
    };
  });
}

const server = createServer(response => {
  if (gate) gate(response);
  else response.writeHead(500).end('Unexpected query');
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
let browser;
try {
  browser = await chromium.launch({ headless: true, executablePath: process.env.CHROMIUM });
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(String(error)));
  await page.goto(`${origin}/?test`);
  await page.waitForFunction(() => typeof window.call === 'function');
  const call = (method, argument) => page.evaluate(([method, argument]) => window.call(method, argument), [method, argument]);
  const url = `${origin}/database-worker.js?name=contract.sqlite3`;
  const tag = [0, 127, 255];

  await call('open', url);
  const created = await page.evaluate(async tag => {
    return Promise.all(Array.from({ length: 20 }, async (_, index) => {
      const name = `Purse ${index}`;
      const id = await window.call('create', { name, tag });
      return { id, name, tag };
    }));
  }, tag);
  assert.equal(new Set(created.map(purse => purse.id)).size, 20);
  assert.deepEqual(await call('list', tag), [...created].sort((left, right) => left.id - right.id));
  const fetched = await page.evaluate(async purses => Promise.all(
    purses.toReversed().map(purse => window.call('get', purse.id))
  ), created);
  assert.deepEqual(fetched, created.toReversed());
  assert.equal(await call('get', 1000), undefined);
  console.log('PASS shared DAO: concurrent creation, unique IDs, typed reads and correlated replies');

  await assert.rejects(call('batch', { names: ['Must roll back', ''], tag }), /refused.*empty/i);
  assert.deepEqual(await call('list', tag), [...created].sort((left, right) => left.id - right.id));
  assert.deepEqual(await call('batch', { names: ['Joint', 'Travel'], tag }), [20, 21]);
  await assert.rejects(call('sql_failure'), /SQLite.*missing_table/i);
  await assert.rejects(call('unknown'), /unknown operation/i);
  assert.deepEqual(await call('get', 20), { id: 20, name: 'Joint', tag });
  console.log('PASS atomic batch rollback, successful batch, SQL errors and unknown-operation rejection');

  const admitted = expectGate();
  let completed = false;
  const held = call('held').then(result => { completed = true; return result; });
  held.catch(() => {});
  const release = await admitted;
  try {
    assert.equal(await call('ping'), 'Rust caller responsive');
    assert.equal(completed, false);
  } finally { release(); }
  await held;
  console.log('PASS isolation: SQLite entered gate -> Rust caller replied -> SQLite released');

  const beforeDrain = await call('list', tag);
  const enteredDrain = expectGate();
  const drainingQuery = call('held');
  drainingQuery.catch(() => {});
  const releaseDrain = await enteredDrain;
  try {
    await page.evaluate(tag => {
      window.drained = Promise.all([
        window.call('create', { name: 'Before close', tag }),
        window.call('close'),
        window.call('close'),
      ]);
    }, tag);
    await assert.rejects(call('get', 0), /closed/i);
  } finally { releaseDrain(); }
  await drainingQuery;
  const [drainedId] = await page.evaluate(() => window.drained);
  await call('close');
  await assert.rejects(call('get', 0), /closed/i);
  await call('open', url);
  assert.deepEqual(await call('list', tag), [...beforeDrain, { id: drainedId, name: 'Before close', tag }]);
  console.log('PASS close drains admitted work, rejects new work, and OPFS survives worker replacement');

  const blocked = expectGate();
  const pending = call('held');
  const rejected = assert.rejects(pending, /outcome unknown/i);
  const unblock = await blocked;
  try { await call('terminate'); await rejected; }
  finally { unblock(); }
  await assert.rejects(call('get', 0), /closed|outcome unknown/i);
  assert.equal(await call('ping'), 'Rust caller responsive');
  console.log('PASS termination settles pending work with unknown outcome and preserves caller');

  await assert.rejects(call('open', `${origin}/missing-worker.js`), /worker failed|outcome unknown/i);
  console.log('PASS worker startup failure rejects its waiting caller');

  await page.goto(origin);
  await page.getByRole('status').filter({ hasText: 'Ready.' }).waitFor();
  await page.getByLabel('Purse name').fill('Browser demo');
  await page.getByRole('button', { name: 'Create purse', exact: true }).click();
  await page.getByRole('button', { name: '0: Browser demo', exact: true }).waitFor();
  await page.getByRole('button', { name: '0: Browser demo', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('#details').textContent.includes('Browser demo'));
  await page.getByRole('button', { name: 'Close and reopen storage' }).click();
  await page.getByRole('status').filter({ hasText: 'Ready.' }).waitFor();
  await page.getByRole('button', { name: '0: Browser demo', exact: true }).waitFor();
  await call('close');
  assert.deepEqual(errors, []);
  console.log('PASS browser UI creates, fetches and lists purses without page errors');
} finally {
  if (browser) await browser.close();
  await new Promise(resolve => server.close(resolve));
}
