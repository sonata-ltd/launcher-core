use super::*;

#[test]
fn output_capture_child_fixture() {
    if std::env::var_os("SONATA_TEST_FORGE_OUTPUT_CAPTURE").is_some() {
        let bytes = [b'x'; 16 * 1024];
        std::io::stdout().write_all(&bytes).unwrap();
        std::io::stderr().write_all(&bytes).unwrap();
    }
}

#[test]
fn the_platform_pipe_finishes_and_keeps_one_truncation_notice() {
    use std::{
        process::Command,
        time::{Duration, Instant},
    };
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("processor.log");
    let (mut capture, stdout, stderr) = OutputCapture::open(&path, 128).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "modloader::forge::output::tests::output_capture_child_fixture",
            "--nocapture",
        ])
        .env("SONATA_TEST_FORGE_OUTPUT_CAPTURE", "1")
        .current_dir(temp.path())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        capture.drain(64 * 1024).unwrap();
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("capture child did not finish");
        }
        std::thread::yield_now();
    }
    capture.drain(64 * 1024).unwrap();
    let body = std::fs::read_to_string(path).unwrap();
    assert_eq!(body.len(), 128);
    assert_eq!(body.matches("[processor log limit reached]").count(), 1);
}
use std::io::Write;

fn texts(lines: &[OutputLine]) -> Vec<&str> {
    lines.iter().map(|line| line.text.as_str()).collect()
}

#[test]
fn partial_lines_keep_identity_and_duplicate_text_gets_new_ids() {
    let mut buffer = OutputBuffer::default();
    buffer.feed(b"Patch");
    let first = buffer.snapshot(false);
    buffer.feed(b"ing\nPatching\n");
    let lines = buffer.snapshot(false);
    assert_eq!(texts(&lines), ["Patching", "Patching"]);
    assert_eq!(first[0].id, lines[0].id);
    assert!(lines[1].id > lines[0].id);
}

#[test]
fn empty_lines_crlf_and_carriage_return_overwrites() {
    let mut buffer = OutputBuffer::default();
    buffer.feed(b"\n10%\r");
    let first = buffer.snapshot(false);
    assert_eq!(texts(&first), ["", "10%"]);
    buffer.feed(b"20%\r");
    let second = buffer.snapshot(false);
    assert_eq!(texts(&second), ["", "20%"]);
    assert_eq!(first[1].id, second[1].id);
    buffer.feed(b"\nDone\r\n");
    assert_eq!(texts(&buffer.snapshot(false)), ["", "20%", "Done"]);
}

#[test]
fn split_utf8_is_retained_and_invalid_bytes_render_lossily() {
    let mut buffer = OutputBuffer::default();
    buffer.feed(&[b'a', 0xd0]);
    assert_eq!(texts(&buffer.snapshot(false)), ["a"]);
    buffer.feed(&[0xaf, 0xff, b'\n']);
    assert_eq!(texts(&buffer.snapshot(false)), ["aЯ�"]);
    buffer.feed(&[0xd0]);
    assert_eq!(texts(&buffer.snapshot(false)), ["aЯ�", ""]);
    assert_eq!(texts(&buffer.snapshot(true)), ["aЯ�", "�"]);
}

#[test]
fn line_and_history_memory_are_bounded() {
    let mut buffer = OutputBuffer::default();
    buffer.feed(&vec![b'x'; MAX_LINE_BYTES * 10]);
    assert_eq!(buffer.lines[0].bytes.len(), MAX_LINE_BYTES);
    let snapshot = buffer.snapshot(false);
    assert!(snapshot[0].text.ends_with('…'));
    buffer.feed(b"\n1\n2\n3\n4\n5\n6\n7\n");
    let snapshot = buffer.snapshot(false);
    assert_eq!(texts(&snapshot), ["2", "3", "4", "5", "6", "7"]);
    assert_eq!(buffer.lines.len(), MAX_LINES);
}

#[test]
fn incremental_reads_only_publish_changed_text_and_final_partial_utf8() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    let mut tail = OutputTail::open(file.path()).unwrap();
    assert!(tail.poll(false).unwrap().is_none());
    file.write_all(b"hello").unwrap();
    let first = tail.poll(false).unwrap().unwrap();
    assert_eq!(texts(&first), ["hello"]);
    assert!(tail.poll(false).unwrap().is_none());
    file.write_all(b" world\n\xd0").unwrap();
    let second = tail.poll(false).unwrap().unwrap();
    assert_eq!(texts(&second), ["hello world", ""]);
    assert_eq!(first[0].id, second[0].id);
    let final_lines = tail.poll(true).unwrap().unwrap();
    assert_eq!(texts(&final_lines), ["hello world", "�"]);
}

#[test]
fn huge_backlog_jumps_to_latest_lines_and_preserves_id_monotonicity() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    let mut tail = OutputTail::open(file.path()).unwrap();
    file.write_all(b"old\n").unwrap();
    let old = tail.poll(false).unwrap().unwrap();
    file.write_all(&vec![b'x'; MAX_READ_BYTES as usize * 3])
        .unwrap();
    file.write_all(b"\n1\n2\n3\n4\n5\n6\n7\n").unwrap();
    let latest = tail.poll(true).unwrap().unwrap();
    assert_eq!(texts(&latest), ["2", "3", "4", "5", "6", "7"]);
    assert!(latest[0].id > old[0].id);
    assert_eq!(tail.offset, file.as_file().metadata().unwrap().len());
}

#[test]
fn huge_single_line_has_a_bounded_preview() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    let mut tail = OutputTail::open(file.path()).unwrap();
    file.write_all(&vec![b'x'; MAX_READ_BYTES as usize * 2])
        .unwrap();
    let snapshot = tail.poll(true).unwrap().unwrap();
    assert_eq!(snapshot.len(), 1);
    assert!(snapshot[0].text.starts_with('…'));
    assert!(snapshot[0].text.len() <= MAX_LINE_BYTES + '…'.len_utf8());
    assert!(tail.poll(true).unwrap().is_none());
}

#[test]
fn truncated_file_resets_parser_but_not_ids() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(b"previous long line\r").unwrap();
    let mut tail = OutputTail::open(file.path()).unwrap();
    let old = tail.poll(false).unwrap().unwrap();
    file.as_file().set_len(0).unwrap();
    file.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
    file.write_all(b"new\n").unwrap();
    let latest = tail.poll(false).unwrap().unwrap();
    assert_eq!(texts(&latest), ["new"]);
    assert!(latest[0].id > old[0].id);
}
