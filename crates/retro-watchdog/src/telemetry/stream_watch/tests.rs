use super::judge::{breaches, Session};
use super::{last_json, locate};
use serde_json::json;

/// The live sample of 2026-11-14 20:48 SGT (Phantom Blade Zero to the iPhone over node .178).
fn live() -> serde_json::Value {
    json!({"gpu_util":97,"enc_util":10,"enc_sessions":1,"enc_fps":60,"enc_latency_us":566,"connected":true,
        "client_ip":"192.0.2.162","connects_10m":1,"connected_at":"2026-11-14 20:47:56.134","viewport":"2670x1878",
        "requested_fps":60,"encoder":"hevc_nvenc","color":"HDR","encoder_flags":"HEVC P4 async 10-bit cbr two-pass rfi spatial-aq",
        "ping_ms":[6,6,12,13,5,6],"nic_tx_mbps":43.6,"nic_rx_mbps":20.9,"top_gpu_process":"PBZ.exe","steam_download_mbps":0,
        "offload_jobs":0,"other_streamers":["GameViewer"]})
}

fn kinds(sample: &serde_json::Value, router: &serde_json::Value) -> Vec<&'static str> {
    breaches(sample, router).into_iter().map(|(k, _)| k).collect()
}

#[test]
fn healthy_lan_sample_only_reports_info_kinds() {
    let router = json!({"band": "5G_2", "rssi": -49});
    assert_eq!(kinds(&live(), &router), vec!["gpu_saturated", "heavy_decode"]);
}

#[test]
fn an_empty_nvenc_window_is_not_low_fps() {
    let mut sample = live();
    sample["enc_fps"] = json!(0);
    sample["enc_latency_us"] = json!(0);
    assert!(!kinds(&sample, &json!({})).contains(&"render_fps_low"));
    let mut session = Session::new("k", 0.0);
    session.add(0.0, &sample, &json!({}));
    session.add(1.0, &live(), &json!({}));
    assert_eq!(session.summary("disconnected")["enc_fps"]["p5"], 60.0);
}

#[test]
fn yuv444_to_the_client_notifies() {
    // The 20:38 session of 2026-11-14 that felt 400-500 ms late.
    let mut sample = live();
    sample["encoder_flags"] = json!("HEVC P4 async yuv444 10-bit cbr two-pass rfi spatial-aq");
    let found = kinds(&sample, &json!({}));
    assert!(found.contains(&"client_yuv444") && !found.contains(&"heavy_decode"), "{found:?}");
    let mut session = Session::new("k", 0.0);
    session.add(0.0, &sample, &json!({}));
    session.add(1.0, &sample, &json!({}));
    let fired: Vec<_> = session.add(2.0, &sample, &json!({})).into_iter().map(|(k, _)| k).collect();
    assert_eq!(fired, vec!["client_yuv444"]);
}

#[test]
fn competing_download_weak_link_and_low_fps_breach() {
    let mut sample = live();
    sample["steam_download_mbps"] = json!(331.3);
    sample["nic_rx_mbps"] = json!(340.0);
    sample["ping_ms"] = json!([8, 140, -1, -1, 30, 12]);
    sample["enc_fps"] = json!(30);
    sample["requested_fps"] = json!(120);
    let found = kinds(&sample, &json!({"band": "2G_1", "rssi": -72}));
    for kind in ["network_rtt", "network_loss", "wifi_signal", "wifi_band_2g", "competing_download", "competing_traffic", "render_fps_low"] {
        assert!(found.contains(&kind), "{kind} missing from {found:?}");
    }
}

#[test]
fn low_fps_in_a_menu_is_not_a_render_breach() {
    let mut sample = live();
    sample["enc_fps"] = json!(66);
    sample["requested_fps"] = json!(120);
    sample["gpu_util"] = json!(35);
    assert!(!kinds(&sample, &json!({})).contains(&"render_fps_low"));
}

#[test]
fn a_breach_notifies_once_after_it_is_sustained() {
    let mut session = Session::new("k", 0.0);
    let mut sample = live();
    sample["steam_download_mbps"] = json!(300);
    let router = json!({"band": "5G_2", "rssi": -49});
    assert!(session.add(0.0, &sample, &router).is_empty());
    assert!(session.add(15_000.0, &sample, &router).is_empty());
    let fired: Vec<_> = session.add(30_000.0, &sample, &router).into_iter().map(|(k, _)| k).collect();
    assert_eq!(fired, vec!["competing_download"], "info kinds never notify");
    assert!(session.add(45_000.0, &sample, &router).is_empty(), "one notification per session");
    let summary = session.summary("disconnected");
    assert_eq!(summary["samples"], 4);
    assert_eq!(summary["ping_loss"]["lost"], 0);
    assert_eq!(summary["breaches"][0]["share_pct"], 100.0);
    assert_eq!(summary["rtt_ms"]["max"], 13.0);
}

#[test]
fn an_interrupted_breach_restarts_its_streak() {
    let mut session = Session::new("k", 0.0);
    let (mut bad, good, router) = (live(), live(), json!({}));
    bad["offload_jobs"] = json!(1);
    session.add(0.0, &bad, &router);
    session.add(1.0, &bad, &router);
    session.add(2.0, &good, &router);
    assert!(session.add(3.0, &bad, &router).is_empty());
}

#[test]
fn probe_output_and_router_list_parse() {
    let out = "#< CLIXML\r\n{\"connected\":false,\"gpu_util\":3}\r\n<Objs Version=\"1.1.0.1\"></Objs>";
    assert_eq!(last_json(out).unwrap()["gpu_util"], 3);
    assert!(last_json("nothing").is_err());
    let router = concat!(r#"{"02:00:00:00:00:10":{"5G_2":{"02:00:00:00:00:12":{"ip":"192.0.2.162","rssi":"-50"}}}}"#, "\n",
        "<RT-BE88U>192.0.2.1>02:00:00:00:00:11>1<RT-BE88U>192.0.2.178>02:00:00:00:00:10>0\n");
    let link = locate(router, "192.0.2.162");
    assert_eq!(link["node_ip"], "192.0.2.178");
    assert_eq!(link["band"], "5G_2");
    assert_eq!(link["rssi"], -50);
    assert!(locate(router, "192.0.2.9")["error"].is_string());
}
