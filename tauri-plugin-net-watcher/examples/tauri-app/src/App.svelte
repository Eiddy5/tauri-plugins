<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import {
    getSnapshot,
    onNetworkUpdated,
    onTargetUpdated,
    startWatching,
    stopWatching,
    type NetWatcherSnapshot,
    type NetworkUpdatedPayload,
    type TargetUpdatedPayload,
  } from 'tauri-plugin-net-watcher-api'

  interface EventNotification {
    receivedAt: string
    snapshotId: string
    kind: 'network' | 'target'
    title: string
    detail: string
  }

  let snapshot: NetWatcherSnapshot | null = $state(null)
  let error = $state('')
  let isBusy = $state(false)
  let isListening = $state(false)
  let notifications: EventNotification[] = $state([])
  let unlistenNetwork: (() => void) | null = null
  let unlistenTarget: (() => void) | null = null

  const primaryInterface = $derived(
    snapshot?.network.interfaces.find((item) => item.id === snapshot?.network.primaryInterfaceId)
      ?? snapshot?.network.interfaces.find((item) => item.isPrimary)
      ?? null,
  )
  const interfaces = $derived(snapshot?.network.interfaces ?? [])
  const reachabilityTargets = $derived(snapshot?.reachability.targets ?? [])
  const reachableTargetCount = $derived(
    reachabilityTargets.filter((item) => item.state.reachability === 'reachable').length,
  )
  const unreachableTargetCount = $derived(
    reachabilityTargets.filter((item) => item.state.reachability === 'unreachable').length,
  )
  const changedFields = $derived(snapshot?.changes.changedFields ?? [])
  const changedTargetIds = $derived(snapshot?.changes.changedTargetIds ?? [])
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
        || value?.reachability.targets.some((item) => item.currentProbe),
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

  function recordNotification(notification: Omit<EventNotification, 'receivedAt'>) {
    notifications = [
      {
        receivedAt: new Date().toISOString(),
        ...notification,
      },
      ...notifications,
    ].slice(0, 20)
  }

  function applyNetworkUpdate(payload: NetworkUpdatedPayload) {
    if (!snapshot) return

    const previousOverall = snapshot.state.overall
    snapshot = {
      ...snapshot,
      meta: {
        ...snapshot.meta,
        snapshotId: payload.snapshotId,
        timestamp: payload.timestamp,
        platform: payload.platform,
      },
      state: payload.state,
      network: payload.network,
      changes: {
        hasChanges: true,
        previousOverall,
        currentOverall: payload.state.overall,
        changedFields: ['network'],
        changedTargetIds: [],
      },
    }
    recordNotification({
      snapshotId: payload.snapshotId,
      kind: 'network',
      title: `internet ${payload.state.internet}`,
      detail: `local ${payload.state.network}`,
    })
    error = ''
  }

  function applyTargetUpdate(payload: TargetUpdatedPayload) {
    if (!snapshot) return

    const currentTargets = snapshot.reachability.targets
    const targetExists = currentTargets.some((item) => item.id === payload.target.id)
    snapshot = {
      ...snapshot,
      meta: {
        ...snapshot.meta,
        snapshotId: payload.snapshotId,
        timestamp: payload.timestamp,
      },
      reachability: {
        ...snapshot.reachability,
        targets: targetExists
          ? currentTargets.map((item) => item.id === payload.target.id ? payload.target : item)
          : [...currentTargets, payload.target],
      },
      changes: {
        hasChanges: true,
        previousOverall: snapshot.state.overall,
        currentOverall: snapshot.state.overall,
        changedFields: ['reachability.targets'],
        changedTargetIds: [payload.target.id],
      },
    }
    recordNotification({
      snapshotId: payload.snapshotId,
      kind: 'target',
      title: payload.target.id,
      detail: `${payload.target.state.reachability} · ${payload.target.state.quality}`,
    })
    error = ''
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

  async function initialize() {
    try {
      [unlistenNetwork, unlistenTarget] = await Promise.all([
        onNetworkUpdated(applyNetworkUpdate),
        onTargetUpdated(applyTargetUpdate),
      ])
      isListening = true
      await start()
    } catch (err) {
      error = formatError(err)
    }
  }

  onMount(() => {
    void initialize()
  })

  onDestroy(() => {
    unlistenNetwork?.()
    unlistenTarget?.()
  })
</script>

<main class="net-watcher">
  <header class="app-header">
    <div>
      <p class="eyebrow">Net Watcher</p>
      <h1>Network monitor</h1>
    </div>
    <div class="header-tools">
      <span class:active={isListening} class="listener">
        {isListening ? 'Listening' : 'Listener offline'}
      </span>
      <div class="actions">
        <button type="button" disabled={isBusy} onclick={start}>Start</button>
        <button type="button" disabled={isBusy} onclick={stop}>Stop</button>
        <button type="button" disabled={isBusy} onclick={refreshSnapshot}>Refresh</button>
      </div>
    </div>
  </header>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  {#if snapshot}
    <section class="network-overview" aria-labelledby="network-overview-title">
      <div class="section-heading">
        <div>
          <p class="section-label">Device network</p>
          <h2 id="network-overview-title">Overall network</h2>
        </div>
        <span class="state-pill" data-state={snapshot.state.internet}>
          internet {snapshot.state.internet}
        </span>
      </div>

      <div class="overview-grid">
        <dl>
          <dt>Device</dt>
          <dd>{snapshot.meta.platform} desktop</dd>
        </dl>
        <dl>
          <dt>Network type</dt>
          <dd>{primaryInterface?.type ?? 'unknown'}</dd>
        </dl>
        <dl>
          <dt>Network condition</dt>
          <dd>{snapshot.state.network}</dd>
        </dl>
        <dl>
          <dt>Public internet</dt>
          <dd>{snapshot.network.internet.status}</dd>
        </dl>
        <dl>
          <dt>Internet verified</dt>
          <dd>{snapshot.network.internet.verified ? 'yes' : 'no'}</dd>
        </dl>
        <dl>
          <dt>Internet latency</dt>
          <dd>{formatLatency(snapshot.network.internet.activeProbe?.durationMs)}</dd>
        </dl>
        <dl>
          <dt>Primary interface</dt>
          <dd>{primaryInterface?.displayName || primaryInterface?.name || 'n/a'}</dd>
        </dl>
        <dl>
          <dt>IPv4</dt>
          <dd>{formatList(primaryInterface?.addresses.ipv4)}</dd>
        </dl>
        <dl>
          <dt>Gateway</dt>
          <dd>{primaryInterface?.gateway ?? 'n/a'}</dd>
        </dl>
        <dl>
          <dt>DNS</dt>
          <dd>{formatList(primaryInterface?.dnsServers)}</dd>
        </dl>
        <dl>
          <dt>System hint</dt>
          <dd>{snapshot.network.internet.systemHint.level}</dd>
        </dl>
        <dl class="overview-reason">
          <dt>Internet reason</dt>
          <dd>{snapshot.network.internet.reason}</dd>
        </dl>
        <dl>
          <dt>Internet checked</dt>
          <dd>{formatDate(snapshot.network.internet.checkedAt)}</dd>
        </dl>
      </div>

      <details class="interface-details">
        <summary>All network interfaces ({interfaces.length})</summary>
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
                      <small>{item.id}</small>
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
      </details>
    </section>

    <section class="targets-section" aria-labelledby="targets-title">
      <div class="section-heading target-heading">
        <div>
          <p class="section-label">Custom probes</p>
          <h2 id="targets-title">Service targets</h2>
        </div>
        <div class="target-counts" aria-label="Target status counts">
          <span>{reachabilityTargets.length} total</span>
          <span class="count-reachable">{reachableTargetCount} reachable</span>
          <span class="count-unreachable">{unreachableTargetCount} unreachable</span>
        </div>
      </div>

      {#if reachabilityTargets.length}
        <div class="target-grid">
          {#each reachabilityTargets as item}
            <article class="target-card" data-state={item.state.reachability}>
              <header class="target-header">
                <div>
                  <p class="target-id">{item.id}</p>
                  <p class="target-url">{item.target.url}</p>
                </div>
                <span class="state-pill compact" data-state={item.state.reachability}>
                  {item.state.reachability}
                </span>
              </header>

              <div class="target-state">
                <div>
                  <span>Quality</span>
                  <strong>{item.state.quality}</strong>
                </div>
                <div>
                  <span>Reason</span>
                  <strong>{item.state.reason}</strong>
                </div>
              </div>

              <div class="metric-grid">
                <dl>
                  <dt>HTTP</dt>
                  <dd>{item.currentProbe?.http?.statusCode ?? 'n/a'}</dd>
                </dl>
                <dl>
                  <dt>Duration</dt>
                  <dd>{formatLatency(item.currentProbe?.durationMs)}</dd>
                </dl>
                <dl>
                  <dt>Failure rate</dt>
                  <dd>{formatPercent(item.summary.failureRate)}</dd>
                </dl>
                <dl>
                  <dt>P95 latency</dt>
                  <dd>{formatLatency(item.summary.latencyMs.p95)}</dd>
                </dl>
                <dl>
                  <dt>Jitter</dt>
                  <dd>{formatLatency(item.summary.jitterMs)}</dd>
                </dl>
                <dl>
                  <dt>Failures</dt>
                  <dd>{item.summary.consecutiveFailures} consecutive</dd>
                </dl>
              </div>

              <div class="probe-phases">
                <span>DNS {formatLatency(item.currentProbe?.phases.dnsMs ?? undefined)}</span>
                <span>TCP {formatLatency(item.currentProbe?.phases.tcpMs ?? undefined)}</span>
                <span>TLS {formatLatency(item.currentProbe?.phases.tlsMs ?? undefined)}</span>
                <span>HTTP {formatLatency(item.currentProbe?.phases.httpMs ?? undefined)}</span>
              </div>

              <dl class="target-history">
                <div>
                  <dt>Samples</dt>
                  <dd>{item.summary.successCount} success / {item.summary.failureCount} failed</dd>
                </div>
                <div>
                  <dt>Last success</dt>
                  <dd>{formatDate(item.summary.lastSuccessAt)}</dd>
                </div>
                <div>
                  <dt>Last failure</dt>
                  <dd>{formatDate(item.summary.lastFailureAt)}</dd>
                </div>
              </dl>

              {#if item.currentProbe?.error}
                <p class="target-error">
                  {item.currentProbe.error.code}: {item.currentProbe.error.message}
                </p>
              {/if}
            </article>
          {/each}
        </div>
      {:else}
        <div class="empty-targets">
          <strong>No service targets configured</strong>
          <span>Device network monitoring remains active.</span>
        </div>
      {/if}
    </section>

    <section class="diagnostics" aria-labelledby="diagnostics-title">
      <div class="section-heading">
        <div>
          <p class="section-label">Test output</p>
          <h2 id="diagnostics-title">Diagnostics</h2>
        </div>
        <span class="snapshot-id">{snapshot.meta.snapshotId}</span>
      </div>

      <div class="diagnostic-grid">
        <details open>
          <summary>Latest changes</summary>
          {#if changedTargetIds.length}
            <p class="muted diagnostic-copy">Targets: {changedTargetIds.join(', ')}</p>
          {/if}
          {#if changedFields.length}
            <ul class="chips">
              {#each changedFields as field}
                <li>{field}</li>
              {/each}
            </ul>
          {:else}
            <p class="muted diagnostic-copy">No semantic changes in this snapshot.</p>
          {/if}
        </details>

        <details>
          <summary>Plugin notifications ({notifications.length})</summary>
          {#if notifications.length}
            <div class="notification-list">
              {#each notifications as item}
                <article class="notification-item">
                  <div>
                    <strong>{item.kind}: {item.title}</strong>
                    <span>{formatDate(item.receivedAt)}</span>
                  </div>
                  <p>{item.detail}</p>
                </article>
              {/each}
            </div>
          {:else}
            <p class="muted diagnostic-copy">No snapshot events received.</p>
          {/if}
        </details>

        <details>
          <summary>Raw snapshot JSON</summary>
          <pre>{rawSnapshot}</pre>
        </details>
      </div>
    </section>
  {:else}
    <section class="loading-state">
      <strong>Waiting for network snapshot</strong>
      <span>The watcher is starting.</span>
    </section>
  {/if}
</main>

<style>
  :global(*) {
    box-sizing: border-box;
  }

  :global(body) {
    margin: 0;
    color: #17212b;
    background: #f4f6f8;
    font-family:
      Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  }

  button,
  summary {
    font: inherit;
  }

  .net-watcher {
    width: min(1320px, 100%);
    min-height: 100vh;
    margin: 0 auto;
    padding: 28px 32px 48px;
  }

  .app-header,
  .header-tools,
  .actions,
  .section-heading,
  .target-header,
  .target-counts,
  .probe-phases,
  .notification-item > div {
    display: flex;
    align-items: center;
  }

  .app-header {
    justify-content: space-between;
    gap: 24px;
    margin-bottom: 24px;
  }

  .header-tools {
    justify-content: flex-end;
    gap: 14px;
    flex-wrap: wrap;
  }

  .actions {
    gap: 8px;
  }

  h1,
  h2,
  p,
  dl {
    margin: 0;
  }

  h1 {
    margin-top: 3px;
    font-size: 28px;
    line-height: 1.2;
  }

  h2 {
    font-size: 19px;
    line-height: 1.3;
  }

  .eyebrow,
  .section-label {
    color: #667480;
    font-size: 12px;
    font-weight: 750;
    text-transform: uppercase;
  }

  button {
    min-width: 78px;
    min-height: 36px;
    padding: 7px 14px;
    border: 1px solid #b9c4cc;
    border-radius: 6px;
    color: #ffffff;
    background: #243442;
    cursor: pointer;
  }

  button:hover {
    background: #17212b;
  }

  button:disabled {
    cursor: wait;
    opacity: 0.55;
  }

  .listener,
  .state-pill,
  .target-counts span {
    display: inline-flex;
    align-items: center;
    min-height: 28px;
    padding: 4px 9px;
    border: 1px solid #d5dde3;
    border-radius: 999px;
    color: #56636e;
    background: #ffffff;
    font-size: 12px;
    font-weight: 750;
    white-space: nowrap;
  }

  .listener.active,
  .state-pill[data-state="online"],
  .state-pill[data-state="available"],
  .state-pill[data-state="reachable"],
  .count-reachable {
    color: #17603a;
    border-color: #9bc8ae;
    background: #edf8f1;
  }

  .state-pill[data-state="offline"],
  .state-pill[data-state="unavailable"],
  .state-pill[data-state="captivePortal"],
  .state-pill[data-state="unreachable"],
  .count-unreachable {
    color: #8b2828;
    border-color: #e5aaaa;
    background: #fff1f1;
  }

  .state-pill[data-state="unknown"] {
    color: #6d5d22;
    border-color: #dacb91;
    background: #fff9e7;
  }

  .state-pill[data-state="degraded"] {
    color: #73510b;
    border-color: #dec16e;
    background: #fff7db;
  }

  .state-pill {
    min-width: 88px;
    justify-content: center;
    text-transform: capitalize;
  }

  .state-pill.compact {
    min-width: 0;
  }

  .error,
  .target-error {
    color: #8b2828;
    border: 1px solid #e5aaaa;
    background: #fff1f1;
  }

  .error {
    margin-bottom: 20px;
    padding: 11px 13px;
    border-radius: 6px;
  }

  .network-overview {
    padding: 24px 0;
    border-top: 1px solid #ccd5dc;
    border-bottom: 1px solid #ccd5dc;
    background: #ffffff;
  }

  .section-heading {
    justify-content: space-between;
    gap: 18px;
    padding: 0 24px;
  }

  .overview-grid {
    display: grid;
    grid-template-columns: repeat(6, minmax(130px, 1fr));
    gap: 0;
    margin-top: 22px;
    border-top: 1px solid #e1e6ea;
    border-bottom: 1px solid #e1e6ea;
  }

  .overview-grid dl {
    min-width: 0;
    padding: 16px 20px;
    border-right: 1px solid #e1e6ea;
  }

  .overview-grid dl:nth-child(6n) {
    border-right: 0;
  }

  dt,
  .target-state span {
    color: #687682;
    font-size: 12px;
    font-weight: 700;
  }

  dd {
    margin: 5px 0 0;
    color: #17212b;
    font-size: 14px;
    font-weight: 650;
    overflow-wrap: anywhere;
  }

  .interface-details {
    margin: 18px 24px 0;
  }

  summary {
    color: #3f4e5a;
    font-size: 13px;
    font-weight: 750;
    cursor: pointer;
  }

  .table-wrap {
    margin-top: 12px;
    overflow-x: auto;
  }

  table {
    width: 100%;
    min-width: 880px;
    border-collapse: collapse;
  }

  th,
  td {
    padding: 9px 11px;
    border-bottom: 1px solid #e1e6ea;
    text-align: left;
    vertical-align: top;
    font-size: 12px;
  }

  th {
    color: #687682;
    background: #f5f7f8;
  }

  td {
    overflow-wrap: anywhere;
  }

  td strong,
  td small {
    display: block;
  }

  td small {
    margin-top: 2px;
    color: #77848f;
  }

  .targets-section,
  .diagnostics {
    margin-top: 32px;
  }

  .target-heading {
    padding: 0;
  }

  .target-counts {
    justify-content: flex-end;
    gap: 7px;
    flex-wrap: wrap;
  }

  .target-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 340px), 1fr));
    align-items: start;
    gap: 16px;
    margin-top: 16px;
  }

  .target-card {
    align-self: start;
    min-width: 0;
    padding: 18px;
    border: 1px solid #ccd5dc;
    border-top: 3px solid #9aa7b1;
    border-radius: 7px;
    background: #ffffff;
  }

  .target-card[data-state="reachable"] {
    border-top-color: #3f9465;
  }

  .target-card[data-state="unreachable"] {
    border-top-color: #c44d4d;
  }

  .target-header {
    align-items: flex-start;
    justify-content: space-between;
    gap: 14px;
  }

  .target-header > div {
    min-width: 0;
  }

  .target-id {
    font-size: 17px;
    font-weight: 800;
    overflow-wrap: anywhere;
  }

  .target-url {
    margin-top: 4px;
    color: #65727d;
    font-size: 12px;
    line-height: 1.45;
    overflow-wrap: anywhere;
  }

  .target-state {
    display: grid;
    grid-template-columns: minmax(90px, 0.8fr) minmax(0, 1.8fr);
    gap: 12px;
    margin: 16px 0;
    padding: 12px 0;
    border-top: 1px solid #e1e6ea;
    border-bottom: 1px solid #e1e6ea;
  }

  .target-state strong {
    display: block;
    margin-top: 3px;
    font-size: 13px;
    overflow-wrap: anywhere;
  }

  .metric-grid {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 8px;
  }

  .metric-grid dl {
    min-width: 0;
    padding: 10px;
    background: #f5f7f8;
    border-radius: 5px;
  }

  .metric-grid dd {
    font-size: 13px;
  }

  .probe-phases {
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 14px;
  }

  .probe-phases span {
    padding: 5px 7px;
    color: #52616d;
    background: #edf1f3;
    border-radius: 4px;
    font-size: 11px;
    font-weight: 650;
  }

  .target-history {
    display: grid;
    gap: 8px;
    margin-top: 15px;
  }

  .target-history div {
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }

  .target-history dd {
    margin: 0;
    max-width: 68%;
    text-align: right;
    font-size: 12px;
  }

  .target-error {
    margin-top: 14px;
    padding: 9px 10px;
    border-radius: 5px;
    font-size: 12px;
    line-height: 1.45;
    overflow-wrap: anywhere;
  }

  .empty-targets,
  .loading-state {
    display: grid;
    gap: 5px;
    margin-top: 16px;
    padding: 28px;
    border: 1px dashed #b9c4cc;
    color: #65727d;
    background: #ffffff;
    text-align: center;
  }

  .empty-targets strong,
  .loading-state strong {
    color: #263643;
  }

  .diagnostics {
    padding-top: 28px;
    border-top: 1px solid #ccd5dc;
  }

  .snapshot-id {
    max-width: 48%;
    color: #687682;
    font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
    font-size: 11px;
    overflow-wrap: anywhere;
    text-align: right;
  }

  .diagnostic-grid {
    display: grid;
    gap: 10px;
    margin-top: 16px;
  }

  .diagnostic-grid > details {
    padding: 13px 15px;
    border: 1px solid #d6dde2;
    background: #ffffff;
  }

  .diagnostic-copy {
    margin-top: 12px;
  }

  .muted {
    color: #687682;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 7px;
    margin: 12px 0 0;
    padding: 0;
    list-style: none;
  }

  .chips li {
    padding: 5px 8px;
    color: #2f5675;
    background: #eef5fa;
    border: 1px solid #c7d9e6;
    border-radius: 4px;
    font-size: 11px;
    font-weight: 700;
  }

  .notification-list {
    display: grid;
    gap: 8px;
    margin-top: 12px;
  }

  .notification-item {
    padding: 10px 12px;
    border-left: 3px solid #7d8b96;
    background: #f5f7f8;
  }

  .notification-item > div {
    justify-content: space-between;
    gap: 12px;
  }

  .notification-item p,
  .notification-item span {
    margin-top: 4px;
    color: #687682;
    font-size: 11px;
  }

  pre {
    max-height: 420px;
    margin: 12px 0 0;
    padding: 14px;
    overflow: auto;
    color: #e8edf1;
    background: #1d2831;
    border-radius: 5px;
    font-size: 11px;
    line-height: 1.55;
  }

  @media (max-width: 980px) {
    .overview-grid {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }

    .overview-grid dl,
    .overview-grid dl:nth-child(6n) {
      border-right: 1px solid #e1e6ea;
      border-bottom: 1px solid #e1e6ea;
    }

    .overview-grid dl:nth-child(2n) {
      border-right: 0;
    }
  }

  @media (max-width: 680px) {
    .net-watcher {
      padding: 20px 16px 36px;
    }

    .app-header,
    .section-heading,
    .target-header {
      align-items: flex-start;
      flex-direction: column;
    }

    .header-tools {
      width: 100%;
      align-items: flex-start;
      justify-content: flex-start;
    }

    .actions {
      width: 100%;
    }

    button {
      flex: 1;
    }

    .section-heading {
      padding: 0 16px;
    }

    .target-heading {
      padding: 0;
    }

    .overview-grid {
      grid-template-columns: 1fr;
    }

    .overview-grid dl,
    .overview-grid dl:nth-child(2n),
    .overview-grid dl:nth-child(6n) {
      border-right: 0;
    }

    .interface-details {
      margin-inline: 16px;
    }

    .target-counts {
      justify-content: flex-start;
    }

    .metric-grid {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }

    .target-state {
      grid-template-columns: 1fr;
    }

    .snapshot-id {
      max-width: 100%;
      text-align: left;
    }
  }
</style>
