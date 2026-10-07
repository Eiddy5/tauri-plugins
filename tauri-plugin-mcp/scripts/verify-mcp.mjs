import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'

// The desktop app creates this file. Never print its credential or put it in a URL.
const path = process.argv[2]
if (!path)
  throw new Error(
    'Usage: node scripts/verify-mcp.mjs /path/to/mcp-connection.json',
  )
const config = JSON.parse(await readFile(path, 'utf8'))
const url = new URL(config.url)
assert.equal(
  url.hostname,
  '127.0.0.1',
  'This check only connects to the local example',
)
assert.equal(url.protocol, 'http:')
const protocol = '2025-11-25'
let session
let id = 0
const headers = () => ({
  ...config.headers,
  'Content-Type': 'application/json',
  Accept: 'application/json, text/event-stream',
  ...(session
    ? { 'Mcp-Session-Id': session, 'MCP-Protocol-Version': protocol }
    : {}),
})
function decode(text) {
  try {
    return JSON.parse(text)
  } catch {
    /* Streamable HTTP may use SSE. */
  }
  const message = text
    .split(/\r?\n\r?\n/)
    .map((frame) =>
      frame
        .split(/\r?\n/)
        .filter((line) => line.startsWith('data:'))
        .map((line) => line.slice(5).trim())
        .join('\n'),
    )
    .filter(Boolean)
    .map((data) => JSON.parse(data))
    .find((message) => message.result !== undefined || message.error)
  assert.ok(message, 'Expected JSON-RPC result in response')
  return message
}
async function rpc(method, params, protocolError) {
  const response = await fetch(url, {
    method: 'POST',
    headers: headers(),
    signal: AbortSignal.timeout(10_000),
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: ++id,
      method,
      ...(params ? { params } : {}),
    }),
  })
  assert.equal(response.status, 200, method + ' HTTP status')
  session = response.headers.get('mcp-session-id') ?? session
  const body = decode(await response.text())
  if (protocolError !== undefined) {
    assert.equal(body.error?.code, protocolError, method + ' expected protocol error')
    return body.error
  }
  assert.equal(body.error, undefined, method + ' protocol error')
  return body.result
}
async function call(name, arguments_, errorCode) {
  const result = await rpc('tools/call', { name, arguments: arguments_ })
  assert.equal(result.isError, !!errorCode, name + ' result status')
  if (errorCode) assert.equal(result.structuredContent.code, errorCode)
  console.log(JSON.stringify({ tool: name, result: result.structuredContent }))
  return result.structuredContent
}
try {
  const anonymous = await fetch(url, {
    method: 'POST',
    signal: AbortSignal.timeout(5000),
  })
  assert.equal(anonymous.status, 401)
  await anonymous.body?.cancel()
  const origin = await fetch(url, {
    method: 'POST',
    headers: { ...headers(), Origin: 'http://untrusted.invalid' },
    signal: AbortSignal.timeout(5000),
  })
  assert.equal(origin.status, 403)
  await origin.body?.cancel()
  const initialized = await rpc('initialize', {
    protocolVersion: protocol,
    capabilities: {},
    clientInfo: { name: 'angular-example-check', version: '1' },
  })
  assert.equal(initialized.protocolVersion, protocol)
  assert.equal(initialized.serverInfo.name, 'tauri-plugin-mcp')
  const notification = await fetch(url, {
    method: 'POST',
    headers: headers(),
    signal: AbortSignal.timeout(5000),
    body: JSON.stringify({
      jsonrpc: '2.0',
      method: 'notifications/initialized',
    }),
  })
  assert.ok(notification.ok)
  await notification.body?.cancel()
  const tools = (await rpc('tools/list')).tools
  for (const tool of tools) {
    assert.deepEqual(tool.annotations, {
      readOnlyHint: true, destructiveHint: false,
      idempotentHint: true, openWorldHint: false,
    })
    assert.equal(tool.policy, undefined, 'Runtime policy must not become MCP metadata')
  }
  assert.deepEqual(tools.map((tool) => tool.name).sort(), [
    'math.add',
    'system.fail',
    'system.wait',
    'text.echo',
  ])
  console.log(
    JSON.stringify({
      protocol,
      serverName: initialized.serverInfo.name,
      tools: tools.map((tool) => tool.name),
      authentication: 'passed',
      origin: 'passed',
    }),
  )
  assert.deepEqual(await call('text.echo', { text: '来自 MCP 的真实调用' }), {
    text: '来自 MCP 的真实调用',
    length: 12,
  })
  const [sum, waited] = await Promise.all([
    call('math.add', { a: 19, b: 23 }),
    call('system.wait', { delayMs: 30 }),
  ])
  assert.deepEqual(sum, { sum: 42 })
  assert.deepEqual(waited, { waitedMs: 30 })
  await call('math.add', { a: 'wrong', b: 1 }, 'INVALID_ARGUMENT')
  assert.equal((await rpc('tools/call', { name: 'unknown', arguments: {} }, -32602)).data.code, 'NOT_FOUND')
  await call('system.fail', {}, 'DEMO_ERROR')
  await call('system.wait', { delayMs: 5000 }, 'TIMEOUT')
  console.log(
    'PASS: real desktop MCP → Rust → Tauri Channel → Angular DI methods',
  )
} finally {
  if (session) {
    const response = await fetch(url, {
      method: 'DELETE',
      headers: headers(),
      signal: AbortSignal.timeout(5000),
    })
    await response.body?.cancel()
  }
}
