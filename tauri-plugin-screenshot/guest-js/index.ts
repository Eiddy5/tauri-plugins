import { invoke } from '@tauri-apps/api/core'

/**
 * Options for a native area-selection screenshot.
 *
 * @since 0.1.0
 */
export interface CaptureOptions {
  /**
   * Minimum accepted selection width in logical pixels.
   * @default 2
   */
  minWidth?: number
  /**
   * Minimum accepted selection height in logical pixels.
   * @default 2
   */
  minHeight?: number
}

/**
 * A physical-pixel region relative to the captured virtual desktop image.
 *
 * @since 0.1.0
 */
export interface CaptureRegion {
  /** Physical-pixel X offset relative to the captured virtual desktop image. */
  x: number
  /** Physical-pixel Y offset relative to the captured virtual desktop image. */
  y: number
  /** Selection width in physical pixels. */
  width: number
  /** Selection height in physical pixels. */
  height: number
}

/**
 * A confirmed PNG screenshot.
 *
 * @since 0.1.0
 */
export interface CapturedScreenshot {
  status: 'captured'
  mimeType: 'image/png'
  /** PNG width in physical pixels. */
  width: number
  /** PNG height in physical pixels. */
  height: number
  region: CaptureRegion
  /** Raw PNG bytes. */
  data: Uint8Array
}

/**
 * Returned when the user cancels the screenshot.
 *
 * @since 0.1.0
 */
export interface CancelledScreenshot {
  status: 'cancelled'
}

/**
 * Result of a completed native screenshot interaction.
 *
 * @since 0.1.0
 */
export type CaptureResult = CapturedScreenshot | CancelledScreenshot

interface CapturedEnvelope {
  status: 'captured'
  captureId: string
  mimeType: 'image/png'
  width: number
  height: number
  region: CaptureRegion
}

interface CancelledEnvelope {
  status: 'cancelled'
}

type CaptureEnvelope = CapturedEnvelope | CancelledEnvelope

/**
 * Starts the native area-selection overlay and resolves after the user confirms
 * or cancels the screenshot.
 *
 * Only one screenshot session can run at a time.
 *
 * @returns The confirmed PNG screenshot, or a cancelled result when the user
 * cancels the interaction.
 *
 * @throws Rejects with a structured `{ code, message, recoverable }` error when
 * capture cannot start or complete.
 *
 * @example
 * ```ts
 * import { captureArea } from 'tauri-plugin-screenshot-api'
 *
 * const result = await captureArea({ minWidth: 8, minHeight: 8 })
 * if (result.status === 'captured') {
 *   console.log(result.width, result.height, result.data)
 * }
 * ```
 *
 * @since 0.1.0
 */
export async function captureArea(
  options: CaptureOptions = {},
): Promise<CaptureResult> {
  const envelope = await invoke<CaptureEnvelope>(
    'plugin:screenshot|capture_area',
    { options },
  )

  if (envelope.status === 'cancelled') {
    return envelope
  }

  const raw = await invoke<ArrayBuffer | Uint8Array>(
    'plugin:screenshot|take_capture',
    { captureId: envelope.captureId },
  )
  const data = raw instanceof Uint8Array ? raw : new Uint8Array(raw)

  return {
    status: 'captured',
    mimeType: envelope.mimeType,
    width: envelope.width,
    height: envelope.height,
    region: envelope.region,
    data,
  }
}
