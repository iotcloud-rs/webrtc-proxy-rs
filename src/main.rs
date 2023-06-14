#[macro_use]
extern crate tracing;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use futures::{Sink, SinkExt, StreamExt};
use once_cell::sync::Lazy;
use retina::client::{PlayOptions, Session, SessionOptions, SetupOptions};
use retina::codec::CodecItem;
use time::OffsetDateTime;
use tokio::spawn;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, tungstenite};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264};
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

use webrtc_proxy::h264_to_sample;
use webrtc_proxy::signaling::{
    Answer, Candidate, Connected, Hello, Offer, Payload, PayloadData, Signal,
};

use crate::trace::tracing_init;

mod trace;

#[derive(Debug, Parser)]
#[command()]
struct Args {
    #[arg(short, long, default_value_t = String::from("ws://127.0.0.1:3000/signaling"))]
    signaling: String,
    #[arg(short, long, default_value_t = String::from("proxy"))]
    peer: String,
}

pub static CONNS: Lazy<Mutex<HashMap<String, Arc<Mutex<RTCPeerConnection>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[tokio::main]
async fn main() {
    let args = Args::parse();
    tracing_init();
    info!("WebRTC Proxy: {:?}", args);
    let peer = args.peer;
    loop {
        if let Ok((ws, _)) = connect_async(&args.signaling).await {
            info!("connect {}", &args.signaling);
            let (tx, mut rx) = ws.split();
            let tx = Arc::new(Mutex::new(tx));

            let msg = Message::text(
                serde_json::to_string(&Signal::Hello(Hello {
                    subject: peer.clone(),
                }))
                .unwrap(),
            );
            tx.lock().await.send(msg).await.unwrap();

            while let Some(Ok(Message::Text(text))) = rx.next().await {
                let sig = serde_json::from_str::<Signal>(&text);
                match sig {
                    Ok(sig) => {
                        trace!("{:?}", sig);
                        match &sig {
                            Signal::Hello(_) => {}
                            Signal::Welcome(_) => {}
                            Signal::Payload(payload) => match &payload.payload {
                                PayloadData::Connect(connect) => {
                                    let session_id = uuid::Uuid::new_v4().to_string();
                                    info!(
                                        "new session from: {} to: {} sid: {}",
                                        payload.from, payload.to, session_id
                                    );
                                    let url = url::Url::parse(connect.url.as_str()).unwrap();
                                    let pc = create_peer_connection(
                                        url,
                                        tx.clone(),
                                        peer.clone(),
                                        payload.from.clone(),
                                        session_id.clone(),
                                    )
                                    .await;
                                    let pc = Arc::new(Mutex::new(pc));
                                    CONNS.lock().await.insert(session_id.clone(), pc.clone());
                                    let msg = Message::text(
                                        serde_json::to_string(&Signal::Payload(Payload {
                                            from: peer.clone(),
                                            to: payload.from.clone(),
                                            session_id: None,
                                            payload: PayloadData::Connected(Connected {
                                                session_id,
                                            }),
                                        }))
                                        .unwrap(),
                                    );
                                    tx.lock().await.send(msg).await.unwrap();
                                }
                                PayloadData::Connected(_) => {}
                                PayloadData::Offer(offer) => {
                                    if let Some(pc) = {
                                        CONNS.lock().await.get(payload.session_id.as_ref().unwrap())
                                    } {
                                        let answer = set_offer(pc, offer).await;
                                        let msg = Message::text(
                                            serde_json::to_string(&Signal::Payload(Payload {
                                                from: peer.clone(),
                                                to: payload.from.clone(),
                                                session_id: payload.session_id.clone(),
                                                payload: PayloadData::Answer(answer),
                                            }))
                                            .unwrap(),
                                        );
                                        tx.lock().await.send(msg).await.unwrap();
                                    }
                                }
                                PayloadData::Answer(_) => {}
                                PayloadData::Candidate(candidate) => {
                                    if let Some(pc) = {
                                        CONNS.lock().await.get(payload.session_id.as_ref().unwrap())
                                    } {
                                        add_candidate(pc, candidate).await;
                                    }
                                }
                            },
                        }
                    }
                    Err(err) => {
                        debug!("{:?}", err);
                    }
                }
            }
        } else {
            error!("failed to connect to {}", &args.signaling);
        }
        sleep(Duration::from_secs(3)).await;
    }
}

async fn create_peer_connection(
    url: url::Url,
    tx: Arc<Mutex<dyn Sink<Message, Error = tungstenite::Error> + Unpin + Send>>,
    local_id: String,
    remote_id: String,
    session_id: String,
) -> RTCPeerConnection {
    let mut media = MediaEngine::default();
    media.register_default_codecs().unwrap();

    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media).unwrap();

    let api = APIBuilder::new()
        .with_media_engine(media)
        .with_interceptor_registry(registry)
        .build();

    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec!["stun:stun.minisipserver.com:3478".to_owned()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let conn = api.new_peer_connection(config).await.unwrap();

    conn.on_negotiation_needed(Box::new(move || {
        trace!("on_negotiation_needed");
        Box::pin(async {})
    }));

    conn.on_signaling_state_change(Box::new(move |state| {
        trace!("on_signaling_state_change {}", state);
        Box::pin(async {})
    }));

    conn.on_ice_gathering_state_change(Box::new(move |state| {
        trace!("on_ice_gathering_state_change {}", state);
        Box::pin(async {})
    }));

    conn.on_ice_connection_state_change(Box::new(move |state| {
        trace!("on_ice_connection_state_change {}", state);
        Box::pin(async {})
    }));

    conn.on_peer_connection_state_change(Box::new(move |state| {
        trace!("on_peer_connection_state_change {}", state);
        Box::pin(async {})
    }));

    let tx2 = tx.clone();
    conn.on_ice_candidate(Box::new(move |candidate| {
        trace!("on_ice_candidate {:?}", candidate);
        let tx3 = tx2.clone();
        let local_id2 = local_id.clone();
        let remote_id2 = remote_id.clone();
        let session_id2 = session_id.clone();
        Box::pin(async move {
            if let Some(c) = candidate {
                let c = c.to_json().unwrap();
                let c = Candidate {
                    candidate: c.candidate.clone(),
                    sdp_mid: c.sdp_mid.clone(),
                    sdp_m_line_index: c.sdp_mline_index,
                    username_fragment: c.username_fragment.clone(),
                };
                let msg = Message::text(
                    serde_json::to_string(&Signal::Payload(Payload {
                        from: local_id2,
                        to: remote_id2,
                        session_id: Some(session_id2),
                        payload: PayloadData::Candidate(c),
                    }))
                    .unwrap(),
                );
                tx3.lock().await.send(msg).await.unwrap();
            }
        })
    }));

    conn.on_track(Box::new(move |track, _rx, _tx| {
        trace!("on_track {:?}", track);
        Box::pin(async {})
    }));

    conn.on_data_channel(Box::new(move |ch| {
        trace!("on_data_channel {} {}", ch.label(), ch.protocol());
        ch.on_open(Box::new(|| {
            trace!("channel open");
            Box::pin(async {})
        }));
        ch.on_close(Box::new(|| {
            trace!("channel close");
            Box::pin(async {})
        }));
        let ch2 = ch.clone();
        ch.on_message(Box::new(move |_msg| {
            let ch3 = ch2.clone();
            Box::pin(async move {
                let format = time::format_description::parse(
                    "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3][offset_hour sign:mandatory]:[offset_minute]",
                )
                    .unwrap();
                let now = OffsetDateTime::now_local()
                    .unwrap()
                    .format(&format)
                    .unwrap();
                ch3.send_text(now).await.unwrap();
            })
        }));
        Box::pin(async {})
    }));

    info!("play {url}");
    let mut session = Session::describe(url, SessionOptions::default())
        .await
        .unwrap();
    for (i, s) in session.streams().iter().enumerate() {
        info!("stream {i} {s:?}");
    }
    session.setup(0, SetupOptions::default()).await.unwrap();
    let playing = session.play(PlayOptions::default()).await.unwrap();
    let mut demuxed = playing.demuxed().unwrap();

    let video_track = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_H264.to_owned(),
            ..Default::default()
        },
        "video".to_owned(),
        "webrtc-rs".to_owned(),
    ));

    let rtp_sender = conn.add_track(video_track.clone()).await.unwrap();

    spawn(async move {
        let mut rtcp_buf = vec![0u8; 1500];
        while let Ok((_, _)) = rtp_sender.read(&mut rtcp_buf).await {}
    });

    spawn(async move {
        while let Some(Ok(item)) = demuxed.next().await {
            if let CodecItem::VideoFrame(frame) = item {
                let sample = h264_to_sample(frame);
                video_track.write_sample(&sample).await.unwrap();
            }
        }
    });
    conn
}

async fn set_offer(pc: &Arc<Mutex<RTCPeerConnection>>, offer: &Offer) -> Answer {
    trace!("set_remote_description");
    let pc = pc.lock().await;
    pc.set_remote_description(RTCSessionDescription::offer(offer.sdp.clone()).unwrap())
        .await
        .unwrap();
    let answer = pc.create_answer(None).await.unwrap();
    trace!("set_local_description");
    pc.set_local_description(answer.clone()).await.unwrap();
    Answer { sdp: answer.sdp }
}

async fn add_candidate(pc: &Arc<Mutex<RTCPeerConnection>>, candidate: &Candidate) {
    let pc = pc.lock().await;
    pc.add_ice_candidate(RTCIceCandidateInit {
        candidate: candidate.candidate.clone(),
        sdp_mid: candidate.sdp_mid.clone(),
        sdp_mline_index: candidate.sdp_m_line_index,
        username_fragment: candidate.username_fragment.clone(),
    })
    .await
    .unwrap();
}
