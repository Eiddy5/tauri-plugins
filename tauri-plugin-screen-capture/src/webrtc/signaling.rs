use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use interceptor::registry::Registry;
use tokio::sync::watch;
use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors,
        media_engine::{MediaEngine, MIME_TYPE_H264},
        APIBuilder,
    },
    ice_transport::ice_candidate::RTCIceCandidateInit,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription, RTCPeerConnection,
    },
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::{track_local_static_sample::TrackLocalStaticSample, TrackLocal},
};

use crate::{
    error::Error,
    models::{CaptureErrorCode, WebRtcAnswer, WebRtcIceCandidate, WebRtcOffer},
    Result,
};

pub struct WebRtcSignalingState {
    peer_connection: Arc<RTCPeerConnection>,
    video_track: Arc<TrackLocalStaticSample>,
    connected_rx: watch::Receiver<bool>,
}

impl WebRtcSignalingState {
    pub async fn new() -> Result<Self> {
        let mut media_engine = MediaEngine::default();
        media_engine
            .register_default_codecs()
            .map_err(|err| webrtc_error("failed to register WebRTC codecs", err))?;
        let registry = register_default_interceptors(Registry::new(), &mut media_engine)
            .map_err(|err| webrtc_error("failed to register WebRTC interceptors", err))?;
        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build();
        let peer_connection = Arc::new(
            api.new_peer_connection(RTCConfiguration::default())
                .await
                .map_err(|err| webrtc_error("failed to create peer connection", err))?,
        );
        let video_track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_H264.to_string(),
                clock_rate: 90_000,
                channels: 0,
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                        .to_string(),
                rtcp_feedback: vec![],
            },
            "video".to_string(),
            "capture".to_string(),
        ));
        let rtp_sender = peer_connection
            .add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|err| webrtc_error("failed to add WebRTC video track", err))?;
        tokio::spawn(async move {
            while let Ok((packets, _)) = rtp_sender.read_rtcp().await {
                for packet in packets {
                    eprintln!("[screen-capture] WebRTC RTCP packet received: {packet:?}");
                }
            }
        });

        let (connected_tx, connected_rx) = watch::channel(false);
        peer_connection.on_peer_connection_state_change(Box::new(move |state| {
            let connected_tx = connected_tx.clone();
            Box::pin(async move {
                eprintln!("[screen-capture] WebRTC peer connection state: {state}");
                if state == RTCPeerConnectionState::Connected {
                    let _ = connected_tx.send(true);
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        }));

        Ok(Self {
            peer_connection,
            video_track,
            connected_rx,
        })
    }

    pub fn video_track(&self) -> Arc<TrackLocalStaticSample> {
        Arc::clone(&self.video_track)
    }

    pub async fn create_offer(&self) -> Result<WebRtcOffer> {
        let offer = self
            .peer_connection
            .create_offer(None)
            .await
            .map_err(|err| webrtc_error("failed to create offer", err))?;
        let mut gathering_complete = self.peer_connection.gathering_complete_promise().await;
        self.peer_connection
            .set_local_description(offer.clone())
            .await
            .map_err(|err| webrtc_error("failed to set local offer", err))?;
        let _ = gathering_complete.recv().await;
        let offer = self
            .peer_connection
            .local_description()
            .await
            .ok_or_else(|| {
                Error::new(
                    CaptureErrorCode::WebRtcNegotiationFailed,
                    "failed to read local WebRTC offer after ICE gathering",
                    true,
                )
            })?;

        Ok(WebRtcOffer {
            sdp: offer.sdp,
            sdp_type: "offer".to_string(),
        })
    }

    pub async fn accept_answer(&self, answer: WebRtcAnswer) -> Result<()> {
        let description = RTCSessionDescription::answer(answer.sdp)
            .map_err(|err| webrtc_error("failed to parse answer", err))?;
        self.peer_connection
            .set_remote_description(description)
            .await
            .map_err(|err| webrtc_error("failed to set remote answer", err))
    }

    pub async fn wait_connected(&self, timeout: Duration) -> bool {
        if *self.connected_rx.borrow() {
            return true;
        }

        let mut connected_rx = self.connected_rx.clone();
        tokio::time::timeout(timeout, async move {
            loop {
                if connected_rx.changed().await.is_err() {
                    return false;
                }
                if *connected_rx.borrow() {
                    return true;
                }
            }
        })
        .await
        .unwrap_or(false)
    }

    pub async fn add_ice_candidate(&self, candidate: WebRtcIceCandidate) -> Result<()> {
        self.peer_connection
            .add_ice_candidate(RTCIceCandidateInit {
                candidate: candidate.candidate,
                sdp_mid: candidate.sdp_mid,
                sdp_mline_index: candidate.sdp_mline_index,
                username_fragment: None,
            })
            .await
            .map_err(|err| webrtc_error("failed to add remote ICE candidate", err))
    }

    pub async fn close(&self) -> Result<()> {
        self.peer_connection
            .close()
            .await
            .map_err(|err| webrtc_error("failed to close peer connection", err))
    }
}

fn webrtc_error(message: &'static str, err: impl std::fmt::Display) -> Error {
    Error::new(
        CaptureErrorCode::WebRtcNegotiationFailed,
        format!("{message}: {err}"),
        true,
    )
}
