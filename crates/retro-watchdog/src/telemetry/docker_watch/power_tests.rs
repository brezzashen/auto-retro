use super::*;
const G: Duration = HANG_GRACE;

#[test]
fn decisions() {
    let late = G + Duration::from_secs(1);
    assert_eq!(decide(true, false, Duration::from_secs(60), G, 0, false), Decision::Wait);
    assert_eq!(decide(true, false, late, G, 0, false), Decision::Wake);
    assert_eq!(decide(true, true, late, G, 0, false), Decision::Held("os_alive_ssh_down"));
    assert_eq!(decide(true, false, late, G, 0, true), Decision::Held("hung_no_power_control"));
    assert_eq!(decide(true, false, late, G, 3, false), Decision::Held("wol_cap_24h"));
    assert_eq!(decide(false, false, late, G, 0, false), Decision::Held("mac_unknown"));
}

#[test]
fn update_window() {
    // 2026-11-01 was a Sunday; 04:10 UTC+8 = 2026-10-31 20:10 UTC.
    assert!(in_update_window(1_793_477_400.0));
    // Monday 04:10 UTC+8.
    assert!(!in_update_window(1_793_477_400.0 + 86_400.0));
}

#[test]
fn mac_from_arp_and_cache() {
    let arp = "? (192.0.2.8) at 02:00:00:00:00:01 on en0 ifscope [ethernet]";
    assert_eq!(parse_mac(arp), Some([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]));
    assert_eq!(parse_mac("02:00:00:00:00:01"), Some([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]));
    assert_eq!(parse_mac("? (192.0.2.8) at (incomplete) on en0"), None);
}

#[test]
fn magic_packet_layout() {
    let mac = [1, 2, 3, 4, 5, 6];
    let packet = magic_packet(&mac);
    assert_eq!(packet.len(), 102);
    assert_eq!(&packet[..6], &[0xFF; 6]);
    assert!(packet[6..].chunks(6).all(|c| c == mac));
}
