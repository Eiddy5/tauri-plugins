use tauri_plugin_screen_capture::webrtc::h264_annexb::take_next_aud_access_unit;

#[test]
fn extracts_one_access_unit_between_aud_nals() {
    let mut stream = vec![
        0, 0, 0, 1, 9, 0xf0, 0, 0, 0, 1, 7, 1, 2, 3, 0, 0, 0, 1, 5, 4, 5, 6, 0, 0, 0, 1, 9, 0xf0,
        0, 0, 0, 1, 1, 7, 8, 9,
    ];

    let access_unit = take_next_aud_access_unit(&mut stream).expect("first access unit");

    assert_eq!(
        access_unit,
        vec![0, 0, 0, 1, 9, 0xf0, 0, 0, 0, 1, 7, 1, 2, 3, 0, 0, 0, 1, 5, 4, 5, 6]
    );
    assert_eq!(stream, vec![0, 0, 0, 1, 9, 0xf0, 0, 0, 0, 1, 1, 7, 8, 9]);
}

#[test]
fn waits_until_the_next_aud_arrives() {
    let mut stream = vec![0, 0, 0, 1, 9, 0xf0, 0, 0, 0, 1, 1, 1, 2, 3];

    assert!(take_next_aud_access_unit(&mut stream).is_none());
    assert_eq!(stream, vec![0, 0, 0, 1, 9, 0xf0, 0, 0, 0, 1, 1, 1, 2, 3]);
}

#[test]
fn drops_junk_before_the_first_start_code() {
    let mut stream = vec![
        99, 100, 0, 0, 0, 1, 9, 0xf0, 0, 0, 0, 1, 1, 1, 2, 3, 0, 0, 0, 1, 9, 0xf0,
    ];

    let access_unit = take_next_aud_access_unit(&mut stream).expect("first access unit");

    assert_eq!(
        access_unit,
        vec![0, 0, 0, 1, 9, 0xf0, 0, 0, 0, 1, 1, 1, 2, 3]
    );
    assert_eq!(stream, vec![0, 0, 0, 1, 9, 0xf0]);
}
