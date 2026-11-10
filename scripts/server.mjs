import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const web = fileURLToPath(new URL('../web/', import.meta.url));

export function createServer(onGate) {
  return http.createServer((request, response) => {
    const url = new URL(request.url, 'http://localhost');
    if (url.pathname === '/query-gate') {
      if (onGate) onGate(response);
      else response.writeHead(400).end('The gate is only available in browser tests');
      return;
    }
    const filename = path.resolve(web, `.${decodeURIComponent(url.pathname === '/' ? '/index.html' : url.pathname)}`);
    if (!filename.startsWith(web) || !fs.existsSync(filename) || !fs.statSync(filename).isFile()) {
      response.writeHead(404).end(); return;
    }
    const types = { '.js': 'text/javascript', '.wasm': 'application/wasm', '.html': 'text/html' };
    response.setHeader('Content-Type', types[path.extname(filename)] ?? 'application/octet-stream');
    response.setHeader('Cache-Control', 'no-store');
    fs.createReadStream(filename).pipe(response);
  });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const port = Number(process.env.PORT ?? 5191);
  createServer().listen(port, '127.0.0.1', () => console.log(`http://127.0.0.1:${port}`));
}
