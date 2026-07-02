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

  function formatPercent(value?: number) {
    if (typeof value !== 'number') return 'n/a'
    return `${Math.round(value * 100)}%`
  }

  function formatLatency(value?: number) {
    if (typeof value !== 'number') return 'n/a'
    return `${Math.round(value)} ms`
  }

  function formatError(value: unknown) {
    if (value instanceof Error) return value.message
    return typeof value === 'string' ? value : JSON.stringify(value)
  }

  async function refreshSnapshot() {
    isBusy = true
    error = ''

    try {
      snapshot = await getSnapshot()
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
      await startWatching()
      await refreshSnapshot()
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
    refreshSnapshot()

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
    .grid {
      grid-template-columns: 1fr;
    }

    dd {
      max-width: 100%;
      text-align: left;
    }
  }
</style>
