<script>
  import {
    checkPermission,
    getCaptureStats,
    listSources,
    pauseCapture,
    requestPermission,
    resumeCapture,
    startCapture,
    stopCapture,
  } from 'tauri-plugin-screen-capture-api'
  import { connectLoopbackVideo } from './lib/screenCapture'

  let sources = []
  let selected = null
  let session = null
  let stats = null
  let error = ''
  let loading = false
  let debugRawSources = false
  let includeCurrentApp = false
  let includeSystemUi = false
  let canvas
  let peerConnection = null
  let pollTimer = null
  let video

  async function refreshSources() {
    loading = true
    error = ''
    try {
      console.info('[screen-capture] refreshing sources')
      sources = await listSources({
        kinds: ['display', 'window'],
        includeThumbnails: true,
        includeCurrentApp,
        includeSystemUi,
        debugRawSources,
      })
      console.info('[screen-capture] sources refreshed', {
        total: sources.length,
        displays: sources.filter((source) => source.kind === 'display').length,
        windows: sources.filter((source) => source.kind === 'window').length,
      })
      if (!selected && sources.length > 0) selected = sources[0]
    } catch (err) {
      error = errorMessage(err)
      console.error('[screen-capture] refresh sources failed', err)
    } finally {
      loading = false
    }
  }

  async function start() {
    if (!selected || session) return
    error = ''
    try {
      const currentPermission = await checkPermission()
      console.info('[screen-capture] permission before request', currentPermission)
      const permission = await requestPermission()
      console.info('[screen-capture] permission after request', permission)
      if (permission !== 'granted') {
        throw new Error(`Screen recording permission is ${permission}`)
      }
      console.info('[screen-capture] starting capture', {
        sourceId: selected.id,
        sourceKind: selected.kind,
        name: selected.name,
      })
      session = await startCapture({
        sourceId: selected.id,
        sourceKind: selected.kind,
        fps: 30,
        width: selected.width,
        height: selected.height,
        captureCursor: true,
        publisher: 'webrtcLoopback',
      })
      console.info('[screen-capture] capture session started', session)
      peerConnection = await connectLoopbackVideo(session, video, canvas)
      console.info('[screen-capture] WebRTC loopback connected')
      pollTimer = setInterval(updateStats, 1000)
      await updateStats()
    } catch (err) {
      error = errorMessage(err)
      console.error('[screen-capture] start failed', err)
      await stop()
    }
  }

  async function updateStats() {
    if (!session) return
    stats = await getCaptureStats(session.sessionId)
    console.info('[screen-capture] stats', stats)
  }

  async function pause() {
    if (!session) return
    await pauseCapture(session.sessionId)
    await updateStats()
  }

  async function resume() {
    if (!session) return
    await resumeCapture(session.sessionId)
    await updateStats()
  }

  async function stop() {
    const activeSession = session
    session = null
    stats = null
    if (pollTimer) clearInterval(pollTimer)
    pollTimer = null
    if (peerConnection) peerConnection.close()
    peerConnection = null
    if (video) video.srcObject = null
    if (activeSession) {
      try {
        await stopCapture(activeSession.sessionId)
      } catch (err) {
        error = errorMessage(err)
      }
    }
  }

  function selectSource(source) {
    if (session) return
    selected = source
  }

  function errorMessage(err) {
    if (err && typeof err === 'object' && 'message' in err) return err.message
    return typeof err === 'string' ? err : JSON.stringify(err)
  }

  $: displays = sources.filter((source) => source.kind === 'display')
  $: windows = sources.filter((source) => source.kind === 'window')
</script>

<main class="app-shell">
  <aside class="sidebar">
    <div class="sidebar-header">
      <h1>Screen Capture</h1>
      <button type="button" on:click={refreshSources} disabled={loading || session}>
        {loading ? 'Refreshing' : 'Refresh'}
      </button>
    </div>

    <div class="options">
      <label><input type="checkbox" bind:checked={debugRawSources} disabled={session} /> Debug raw</label>
      <label><input type="checkbox" bind:checked={includeCurrentApp} disabled={session} /> Current app</label>
      <label><input type="checkbox" bind:checked={includeSystemUi} disabled={session} /> System UI</label>
    </div>

    <section>
      <h2>Displays</h2>
      {#each displays as source}
        <button
          type="button"
          class:selected={selected?.id === source.id}
          class="source-button"
          on:click={() => selectSource(source)}
        >
          <strong>{source.name}</strong>
          <span>{source.width} x {source.height}</span>
        </button>
      {/each}
    </section>

    <section>
      <h2>Windows</h2>
      {#each windows as source}
        <button
          type="button"
          class:selected={selected?.id === source.id}
          class="source-button"
          on:click={() => selectSource(source)}
        >
          <strong>{source.name}</strong>
          <span>{source.appName ?? 'Application'} · {source.width} x {source.height}</span>
          {#if source.filteredReason}<em>{source.filteredReason}</em>{/if}
        </button>
      {/each}
    </section>
  </aside>

  <section class="preview">
    <div class="toolbar">
      <div>
        <strong>{selected ? selected.name : 'No source selected'}</strong>
        <span>{session ? session.status : 'idle'}</span>
      </div>
      <div class="actions">
        <button type="button" on:click={start} disabled={!selected || session}>Start</button>
        <button type="button" on:click={pause} disabled={!session}>Pause</button>
        <button type="button" on:click={resume} disabled={!session}>Resume</button>
        <button type="button" on:click={stop} disabled={!session}>Stop</button>
      </div>
    </div>

    <div class="canvas-wrap">
      <video bind:this={video} autoplay playsinline muted></video>
      <canvas bind:this={canvas}></canvas>
    </div>

    <div class="status-strip">
      <span>Captured {stats?.framesCaptured ?? 0}</span>
      <span>Published {stats?.framesPublished ?? 0}</span>
      <span>Dropped {stats?.framesDropped ?? 0}</span>
      <span>{stats?.started ? 'Streaming' : 'Stopped'}</span>
    </div>

    {#if error}
      <div class="error">{error}</div>
    {/if}
  </section>
</main>
