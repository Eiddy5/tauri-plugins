<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import {
    getSnapshot,
    onSnapshotUpdated,
    startWatching,
    stopWatching,
    type NetWatcherSnapshot,
  } from 'tauri-plugin-net-watcher-api'

  let snapshot: NetWatcherSnapshot | null = $state(null)
  let error = $state('')
  let isBusy = $state(false)
  let isListening = $state(false)
  let unlisten: (() => void) | null = null

  const primaryInterface = $derived(
    snapshot?.network.interfaces.find((item) => item.id === snapshot?.network.primaryInterfaceId)
      ?? snapshot?.network.interfaces.find((item) => item.isPrimary)
      ?? null,
  )
  const interfaces = $derived(snapshot?.network.interfaces ?? [])
  const currentProbe = $derived(snapshot?.quality.currentProbe ?? null)
  const changedFields = $derived(snapshot?.changes.changedFields ?? [])
  const rawSnapshot = $derived(snapshot ? JSON.stringify(snapshot, null, 2) : '')

  function formatPercent(value?: number) {
    if (typeof value !== 'number') return 'n/a'
    return `${Math.round(value * 100)}%`
  }

  function formatLatency(value?: number) {
    if (typeof value !== 'number') return 'n/a'
    return `${Math.round(value)} ms`
  }

  function formatDate(value?: string | null) {
    if (!value) return 'n/a'
    return new Date(value).toLocaleString()
  }

  function formatList(values?: string[]) {
    return values?.length ? values.join(', ') : 'n/a'
  }

  function formatError(value: unknown) {
    if (value instanceof Error) return value.message
    return typeof value === 'string' ? value : JSON.stringify(value)
  }

  function hasObservedData(value: NetWatcherSnapshot | null) {
    return Boolean(
      value?.network.interfaces.length
        || value?.quality.currentProbe
        || value?.quality.summary.sampleCount,
    )
  }

  function isAlreadyWatchingError(value: unknown) {
    return Boolean(
      value
        && typeof value === 'object'
        && 'code' in value
        && value.code === 'already_watching',
    )
  }

  function sleep(ms: number) {
    return new Promise((resolve) => setTimeout(resolve, ms))
  }

  async function loadSnapshot() {
    snapshot = await getSnapshot()
    return snapshot
  }

  async function waitForObservedData() {
    const deadline = Date.now() + 5_000

    while (Date.now() < deadline) {
      const nextSnapshot = await loadSnapshot()

      if (hasObservedData(nextSnapshot)) {
        return
      }

      await sleep(250)
    }
  }

  async function refreshSnapshot() {
    isBusy = true
    error = ''

    try {
      await loadSnapshot()
    } catch (err) {
      error = formatError(err)
    } finally {
      isBusy = false
    }
  }

  async function start() {
    isBusy = true
    error = ''

    try {
      try {
        await startWatching()
      } catch (err) {
        if (!isAlreadyWatchingError(err)) {
          throw err
        }
      }

      await waitForObservedData()
    } catch (err) {
      error = formatError(err)
    } finally {
      isBusy = false
    }
  }

  async function stop() {
    isBusy = true
    error = ''

    try {
      await stopWatching()
      await refreshSnapshot()
    } catch (err) {
      error = formatError(err)
    } finally {
      isBusy = false
    }
  }

  onMount(() => {
    onSnapshotUpdated((nextSnapshot) => {
      snapshot = nextSnapshot
      error = ''
    })
      .then((cleanup) => {
        unlisten = cleanup
        isListening = true
      })
      .catch((err) => {
        error = formatError(err)
      })

    start()
  })

  onDestroy(() => {
    unlisten?.()
  })
</script>

<main class="net-watcher">
  <section class="panel" aria-labelledby="title">
    <div class="header">
      <div>
        <p class="eyebrow">Net Watcher</p>
        <h1 id="title">Network status</h1>
      </div>
      <div class:active={isListening} class="listener">
        {isListening ? 'Listening' : 'Listener offline'}
      </div>
    </div>

    <div class="actions">
      <button type="button" disabled={isBusy} onclick={start}>Start</button>
      <button type="button" disabled={isBusy} onclick={stop}>Stop</button>
      <button type="button" disabled={isBusy} onclick={refreshSnapshot}>Refresh</button>
    </div>

    {#if error}
      <p class="error">{error}</p>
    {/if}

    {#if snapshot}
      <div class="status">
        <div>
          <span>Overall</span>
          <strong>{snapshot.state.overall}</strong>
        </div>
        <div>
          <span>Score</span>
          <strong>{snapshot.state.score}</strong>
        </div>
        <div class="reason">
          <span>Reason</span>
          <strong>{snapshot.state.reason}</strong>
        </div>
      </div>

      <div class="grid">
        <article>
          <h2>Probe target</h2>
          <dl>
            <div>
              <dt>URL</dt>
              <dd>{snapshot.quality.target.url}</dd>
            </div>
            <div>
              <dt>Failure rate</dt>
              <dd>{formatPercent(snapshot.quality.summary.failureRate)}</dd>
            </div>
            <div>
              <dt>P95 latency</dt>
              <dd>{formatLatency(snapshot.quality.summary.latencyMs.p95)}</dd>
            </div>
          </dl>
        </article>

        <article>
          <h2>Primary interface</h2>
          {#if primaryInterface}
            <dl>
              <div>
                <dt>Name</dt>
                <dd>{primaryInterface.displayName || primaryInterface.name}</dd>
              </div>
              <div>
                <dt>Type</dt>
                <dd>{primaryInterface.type}</dd>
              </div>
              <div>
                <dt>Status</dt>
                <dd>{primaryInterface.status}</dd>
              </div>
              <div>
                <dt>IPv4</dt>
                <dd>{primaryInterface.addresses.ipv4.join(', ') || 'n/a'}</dd>
              </div>
            </dl>
          {:else}
            <p class="muted">No primary interface reported.</p>
          {/if}
        </article>
      </div>

      <section class="data-section" aria-labelledby="snapshot-meta-title">
        <h2 id="snapshot-meta-title">Snapshot data</h2>
        <div class="data-grid">
          <dl>
            <div>
              <dt>Snapshot ID</dt>
              <dd>{snapshot.meta.snapshotId}</dd>
            </div>
            <div>
              <dt>Timestamp</dt>
              <dd>{formatDate(snapshot.meta.timestamp)}</dd>
            </div>
            <div>
              <dt>Platform</dt>
              <dd>{snapshot.meta.platform}</dd>
            </div>
            <div>
              <dt>Plugin version</dt>
              <dd>{snapshot.meta.pluginVersion}</dd>
            </div>
          </dl>

          <dl>
            <div>
              <dt>Network state</dt>
              <dd>{snapshot.state.network}</dd>
            </div>
            <div>
              <dt>Quality state</dt>
              <dd>{snapshot.state.quality}</dd>
            </div>
            <div>
              <dt>Primary interface ID</dt>
              <dd>{snapshot.network.primaryInterfaceId ?? 'n/a'}</dd>
            </div>
            <div>
              <dt>Changed</dt>
              <dd>{snapshot.changes.hasChanges ? 'yes' : 'no'}</dd>
            </div>
          </dl>
        </div>
      </section>

      <section class="data-section" aria-labelledby="interfaces-title">
        <h2 id="interfaces-title">Detected interfaces ({interfaces.length})</h2>
        {#if interfaces.length}
          <div class="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Type</th>
                  <th>Status</th>
                  <th>Primary</th>
                  <th>IPv4</th>
                  <th>IPv6</th>
                  <th>Gateway</th>
                  <th>DNS</th>
                </tr>
              </thead>
              <tbody>
                {#each interfaces as item}
                  <tr>
                    <td>
                      <strong>{item.displayName || item.name}</strong>
                      <span>{item.id}</span>
                    </td>
                    <td>{item.type}</td>
                    <td>{item.status}</td>
                    <td>{item.isPrimary ? 'yes' : 'no'}</td>
                    <td>{formatList(item.addresses.ipv4)}</td>
                    <td>{formatList(item.addresses.ipv6)}</td>
                    <td>{item.gateway ?? 'n/a'}</td>
                    <td>{formatList(item.dnsServers)}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {:else}
          <p class="muted">No network interfaces reported.</p>
        {/if}
      </section>

      <div class="grid">
        <article>
          <h2>Current probe</h2>
          {#if currentProbe}
            <dl>
              <div>
                <dt>ID</dt>
                <dd>{currentProbe.id}</dd>
              </div>
              <div>
                <dt>Status</dt>
                <dd>{currentProbe.status}</dd>
              </div>
              <div>
                <dt>Duration</dt>
                <dd>{formatLatency(currentProbe.durationMs)}</dd>
              </div>
              <div>
                <dt>Started</dt>
                <dd>{formatDate(currentProbe.startedAt)}</dd>
              </div>
              <div>
                <dt>Ended</dt>
                <dd>{formatDate(currentProbe.endedAt)}</dd>
              </div>
              <div>
                <dt>HTTP status</dt>
                <dd>{currentProbe.http?.statusCode ?? 'n/a'}</dd>
              </div>
              <div>
                <dt>DNS / TCP / TLS / HTTP</dt>
                <dd>
                  {formatLatency(currentProbe.phases.dnsMs ?? undefined)} /
                  {formatLatency(currentProbe.phases.tcpMs ?? undefined)} /
                  {formatLatency(currentProbe.phases.tlsMs ?? undefined)} /
                  {formatLatency(currentProbe.phases.httpMs ?? undefined)}
                </dd>
              </div>
              <div>
                <dt>Error</dt>
                <dd>{currentProbe.error ? `${currentProbe.error.code}: ${currentProbe.error.message}` : 'n/a'}</dd>
              </div>
            </dl>
          {:else}
            <p class="muted">Waiting for the first probe result.</p>
          {/if}
        </article>

        <article>
          <h2>Quality summary</h2>
          <dl>
            <div>
              <dt>Samples</dt>
              <dd>{snapshot.quality.summary.sampleCount}</dd>
            </div>
            <div>
              <dt>Success / failure</dt>
              <dd>{snapshot.quality.summary.successCount} / {snapshot.quality.summary.failureCount}</dd>
            </div>
            <div>
              <dt>Failure rate</dt>
              <dd>{formatPercent(snapshot.quality.summary.failureRate)}</dd>
            </div>
            <div>
              <dt>Latency avg/min/max/p95</dt>
              <dd>
                {formatLatency(snapshot.quality.summary.latencyMs.avg)} /
                {formatLatency(snapshot.quality.summary.latencyMs.min)} /
                {formatLatency(snapshot.quality.summary.latencyMs.max)} /
                {formatLatency(snapshot.quality.summary.latencyMs.p95)}
              </dd>
            </div>
            <div>
              <dt>Jitter</dt>
              <dd>{formatLatency(snapshot.quality.summary.jitterMs)}</dd>
            </div>
            <div>
              <dt>Consecutive failures</dt>
              <dd>{snapshot.quality.summary.consecutiveFailures}</dd>
            </div>
            <div>
              <dt>Last success</dt>
              <dd>{formatDate(snapshot.quality.summary.lastSuccessAt)}</dd>
            </div>
            <div>
              <dt>Last failure</dt>
              <dd>{formatDate(snapshot.quality.summary.lastFailureAt)}</dd>
            </div>
            <div>
              <dt>Last failure reason</dt>
              <dd>{snapshot.quality.summary.lastFailureReason ?? 'n/a'}</dd>
            </div>
          </dl>
        </article>
      </div>

      <section class="data-section" aria-labelledby="changes-title">
        <h2 id="changes-title">Changed fields</h2>
        {#if changedFields.length}
          <ul class="chips">
            {#each changedFields as field}
              <li>{field}</li>
            {/each}
          </ul>
        {:else}
          <p class="muted">No semantic changes reported in the latest snapshot.</p>
        {/if}
      </section>

      <section class="data-section" aria-labelledby="raw-title">
        <h2 id="raw-title">Raw snapshot JSON</h2>
        <pre>{rawSnapshot}</pre>
      </section>
    {:else}
      <p class="muted">No snapshot loaded yet.</p>
    {/if}
  </section>
</main>

<style>
  :global(body) {
    margin: 0;
  }

  .net-watcher {
    min-height: 100vh;
    padding: 32px;
    color: #17202a;
    background: #eef2f6;
    box-sizing: border-box;
  }

  .panel {
    width: min(960px, 100%);
    margin: 0 auto;
    padding: 28px;
    background: #ffffff;
    border: 1px solid #d8e0e8;
    border-radius: 8px;
    box-shadow: 0 12px 32px rgba(23, 32, 42, 0.08);
    box-sizing: border-box;
  }

  .header,
  .actions,
  .status,
  .grid,
  dl div {
    display: flex;
    gap: 12px;
  }

  .header {
    align-items: flex-start;
    justify-content: space-between;
    margin-bottom: 24px;
  }

  .eyebrow,
  h1,
  h2,
  p,
  dl {
    margin: 0;
  }

  .eyebrow {
    color: #5f6f7d;
    font-size: 13px;
    font-weight: 700;
    text-transform: uppercase;
  }

  h1 {
    margin-top: 4px;
    font-size: 32px;
    line-height: 1.2;
    text-align: left;
  }

  h2 {
    margin-bottom: 16px;
    font-size: 18px;
  }

  .listener {
    padding: 6px 10px;
    color: #7b341e;
    background: #fff7ed;
    border: 1px solid #fed7aa;
    border-radius: 999px;
    font-size: 13px;
    font-weight: 700;
    white-space: nowrap;
  }

  .listener.active {
    color: #14532d;
    background: #f0fdf4;
    border-color: #bbf7d0;
  }

  .actions {
    flex-wrap: wrap;
    margin-bottom: 24px;
  }

  button {
    min-width: 96px;
    border: 1px solid #b8c4d0;
    background: #17202a;
    color: #ffffff;
  }

  button:disabled {
    cursor: wait;
    opacity: 0.65;
  }

  .error {
    margin-bottom: 18px;
    padding: 12px 14px;
    color: #7f1d1d;
    background: #fef2f2;
    border: 1px solid #fecaca;
    border-radius: 8px;
  }

  .status {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    margin-bottom: 20px;
  }

  .status > div,
  article {
    padding: 18px;
    border: 1px solid #d8e0e8;
    border-radius: 8px;
    background: #f8fafc;
  }

  .status span,
  dt {
    color: #5f6f7d;
    font-size: 13px;
    font-weight: 700;
  }

  .status strong {
    display: block;
    margin-top: 4px;
    font-size: 22px;
    line-height: 1.25;
    overflow-wrap: anywhere;
  }

  .status .reason {
    grid-column: span 1;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    margin-bottom: 20px;
  }

  dl {
    display: grid;
    gap: 12px;
  }

  dl div {
    justify-content: space-between;
    border-bottom: 1px solid #e2e8f0;
    padding-bottom: 10px;
  }

  dl div:last-child {
    border-bottom: 0;
    padding-bottom: 0;
  }

  dd {
    margin: 0;
    max-width: 65%;
    text-align: right;
    overflow-wrap: anywhere;
  }

  .muted {
    color: #5f6f7d;
  }

  .data-section {
    margin-top: 20px;
    padding: 18px;
    border: 1px solid #d8e0e8;
    border-radius: 8px;
    background: #ffffff;
  }

  .data-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 20px;
  }

  .table-wrap {
    overflow-x: auto;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    min-width: 920px;
  }

  th,
  td {
    padding: 10px 12px;
    border-bottom: 1px solid #e2e8f0;
    text-align: left;
    vertical-align: top;
    font-size: 13px;
  }

  th {
    color: #5f6f7d;
    font-weight: 800;
    background: #f8fafc;
  }

  td {
    overflow-wrap: anywhere;
  }

  td strong,
  td span {
    display: block;
  }

  td span {
    margin-top: 2px;
    color: #5f6f7d;
    font-size: 12px;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .chips li {
    padding: 6px 10px;
    color: #17406d;
    background: #eff6ff;
    border: 1px solid #bfdbfe;
    border-radius: 999px;
    font-size: 13px;
    font-weight: 700;
  }

  pre {
    max-height: 360px;
    margin: 0;
    padding: 14px;
    overflow: auto;
    color: #dbeafe;
    background: #111827;
    border-radius: 8px;
    font-size: 12px;
    line-height: 1.5;
  }

  @media (max-width: 720px) {
    .net-watcher {
      padding: 16px;
    }

    .panel {
      padding: 20px;
    }

    .header,
    dl div {
      flex-direction: column;
      align-items: flex-start;
    }

    .status,
    .grid,
    .data-grid {
      grid-template-columns: 1fr;
    }

    dd {
      max-width: 100%;
      text-align: left;
    }
  }
</style>
