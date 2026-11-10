export async function connect() {
  const worker = new Worker('/caller-worker.js', { type: 'module' });
  let nextId = 0;
  let failed;
  const pending = new Map();
  function fail(error) {
    failed = error;
    for (const request of pending.values()) {
      clearTimeout(request.timeout);
      request.reject(error);
    }
    pending.clear();
    worker.terminate();
  }
  function wait(id) {
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => fail(new Error('Caller worker timed out')), 20000);
      pending.set(id, { resolve, reject, timeout });
    });
  }
  const ready = wait(0);
  worker.onerror = event => fail(new Error(event.message));
  worker.onmessageerror = () => fail(new Error('Caller message could not be decoded'));
  worker.onmessage = ({ data }) => {
    const id = data.ready ? 0 : data.id;
    const request = pending.get(id);
    if (!request) { fail(new Error(`Unexpected caller reply: ${id}`)); return; }
    clearTimeout(request.timeout);
    pending.delete(id);
    if (data.error) request.reject(new Error(data.error));
    else request.resolve(data.result);
  };
  await ready;
  return (method, argument) => {
    if (failed) return Promise.reject(failed);
    const id = ++nextId;
    const reply = wait(id);
    try { worker.postMessage({ id, method, argument }); }
    catch (error) { fail(error); }
    return reply;
  };
}
