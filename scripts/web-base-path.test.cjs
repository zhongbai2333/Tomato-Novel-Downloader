const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../src/ui/web/templates/app.js'), 'utf8');
const autoBoot = 'boot().catch(err => console.error(err));';
assert.equal(source.split(autoBoot).length, 2);

function loadApp(prefix, library = { items: [] }) {
  const requests = [];
  const rows = [];
  const context = vm.createContext({
    URLSearchParams,
    localStorage: { getItem: () => null },
    document: {
      documentElement: { dataset: { basePath: prefix } },
      getElementById: id => id === 'libraryBody'
        ? { innerHTML: '', appendChild: row => rows.push(row.innerHTML) }
        : null,
      createElement: () => ({ innerHTML: '' }),
    },
    fetch: async (url, options) => {
      requests.push({ url, options });
      return {
        ok: true,
        status: 200,
        headers: { get: () => 'application/json' },
        json: async () => library,
      };
    },
  });
  // Exercise the shipped request/rendering functions without starting UI event wiring or timers.
  vm.runInContext(source.replace(autoBoot, ''), context);
  return { context, requests, rows };
}

for (const prefix of ['', '/tomato', '/apps/tomato']) {
  test(`API requests retain the deployment prefix ${JSON.stringify(prefix)}`, async () => {
    const { context, requests } = loadApp(prefix);
    for (const endpoint of ['/api/status', '/api/search?q=a%20b', '/api/preview/123/cleanup']) {
      await context.fetchWithCreds(endpoint, { method: 'POST', body: '{}' });
      const request = requests.at(-1);
      assert.equal(request.url, `${prefix}${endpoint}`);
      assert.equal(request.options.credentials, 'same-origin');
      assert.equal(request.options.method, 'POST');
      assert.equal(request.options.body, '{}');
    }
  });

  test(`library links retain the prefix and filename encoding ${JSON.stringify(prefix)}`, async () => {
    const { context, requests, rows } = loadApp(prefix, {
      items: [
        { kind: 'file', name: 'book', rel_path: 'my books/番茄 #1.txt', size: 11 },
        { kind: 'dir', name: 'audio', rel_path: 'my books/audio' },
      ],
    });
    await context.refreshLibrary(false);
    assert.equal(requests[0].url, `${prefix}/api/library?start=false`);
    const encoded = 'my%20books/%E7%95%AA%E8%8C%84%20%231.txt';
    assert.ok(rows[0].includes(`href="${prefix}/download/${encoded}"`));
    assert.ok(rows[1].includes(`href="${prefix}/download-zip/my%20books/audio"`));
  });
}
