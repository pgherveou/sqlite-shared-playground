import { connect } from './client.js';

const status = document.querySelector('#status');
const list = document.querySelector('#purses');
const details = document.querySelector('#details');
const tag = [1, 2, 3];
const url = new URL('/database-worker.js?name=demo.sqlite3', location.href).href;
const call = await connect();
window.call = call;

async function refresh() {
  const purses = await call('list', tag);
  list.replaceChildren();
  for (const purse of purses) {
    const item = document.createElement('li');
    const button = document.createElement('button');
    button.textContent = `${purse.id}: ${purse.name}`;
    button.onclick = () => run(async () => {
      details.textContent = JSON.stringify(await call('get', purse.id), null, 2);
    });
    item.append(button);
    list.append(item);
  }
}

async function run(action) {
  status.textContent = 'Working…';
  try { await action(); status.textContent = 'Ready. Data is stored in OPFS.'; }
  catch (error) { status.textContent = String(error); }
}

document.querySelector('#create').onsubmit = event => {
  event.preventDefault();
  run(async () => {
    await call('create', { name: document.querySelector('#name').value, tag });
    await refresh();
  });
};
document.querySelector('#refresh').onclick = () => run(refresh);
document.querySelector('#reopen').onclick = () => run(async () => {
  await call('close');
  await call('open', url);
  await refresh();
});
document.querySelector('#rollback').onclick = () => run(async () => {
  try { await call('batch', { names: ['Must roll back', ''], tag }); }
  finally { await refresh(); }
});

if (!new URL(location.href).searchParams.has('test')) {
  await run(async () => { await call('open', url); await refresh(); });
} else {
  status.textContent = 'Test caller ready';
}
