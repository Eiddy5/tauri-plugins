<script>
  import { captureArea } from 'tauri-plugin-screenshot-api'

  let previewUrl = $state('')
  let status = $state('点击按钮开始原生区域截图')
  let region = $state('')
  let capturing = $state(false)

  async function startCapture() {
    if (capturing) return

    capturing = true
    status = '正在等待框选…'

    try {
      const result = await captureArea()

      if (result.status === 'cancelled') {
        status = '已取消截图'
        region = ''
        return
      }

      if (previewUrl) URL.revokeObjectURL(previewUrl)
      previewUrl = URL.createObjectURL(
        new Blob([result.data], { type: result.mimeType }),
      )
      region = `${result.region.x}, ${result.region.y} · ${result.width} × ${result.height} px`
      status = '截图完成'
    } catch (error) {
      status =
        typeof error === 'object' && error && 'message' in error
          ? String(error.message)
          : String(error)
    } finally {
      capturing = false
    }
  }

</script>

<main>
  <section class="hero">
    <span class="eyebrow">TAURI NATIVE SCREENSHOT</span>
    <h1>像微信一样，框选并确认截图</h1>
    <p>屏幕采集、浮层和确认工具条均由系统原生窗口实现。</p>
    <button onclick={startCapture} disabled={capturing}>
      {capturing ? '截图中…' : '开始截图'}
    </button>
    <div class="status" aria-live="polite">
      <strong>{status}</strong>
      {#if region}<span>{region}</span>{/if}
    </div>
  </section>

  <div class="preview">
    {#if previewUrl}
      <img src={previewUrl} alt="刚刚截取的区域" />
    {:else}
      <div class="placeholder">
        <span>⌗</span>
        <p>截图确认后会显示在这里</p>
      </div>
    {/if}
  </div>
</main>
