import AgoraRTC from "agora-rtc-sdk-ng"

export function normalizeAgoraConfig(config) {
  const appId = config.appId.trim()
  const channel = config.channel.trim()
  const token = config.token.trim() || null
  const uidText = config.uid.trim()
  const uid = uidText ? Number(uidText) : null

  if (!appId) throw new Error("Agora App ID is required")
  if (!channel) throw new Error("Agora channel is required")
  if (uidText && (!Number.isInteger(uid) || uid < 0)) {
    throw new Error("Agora UID must be a non-negative integer")
  }

  return { appId, channel, token, uid }
}

export async function publishAgoraScreenTrack(config, mediaStreamTrack) {
  if (!mediaStreamTrack || mediaStreamTrack.readyState === "ended") {
    throw new Error("Capture video track is not ready")
  }

  const normalized = normalizeAgoraConfig(config)
  const client = AgoraRTC.createClient({ mode: "rtc", codec: "vp8" })
  const localTrack = AgoraRTC.createCustomVideoTrack({
    mediaStreamTrack,
    optimizationMode: "detail",
  })

  let joined = false
  try {
    const uid = await client.join(
      normalized.appId,
      normalized.channel,
      normalized.token,
      normalized.uid,
    )
    joined = true
    await client.publish(localTrack)
    return new AgoraPublication(client, localTrack, uid, normalized.channel)
  } catch (error) {
    localTrack.close()
    if (joined) {
      await client.leave().catch(() => {})
    }
    throw error
  }
}

class AgoraPublication {
  constructor(client, localTrack, uid, channel) {
    this.client = client
    this.localTrack = localTrack
    this.uid = uid
    this.channel = channel
  }

  async stop() {
    await this.client.unpublish(this.localTrack).catch(() => {})
    this.localTrack.close()
    await this.client.leave()
  }
}
