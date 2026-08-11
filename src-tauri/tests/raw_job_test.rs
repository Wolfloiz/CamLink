//! Roteamento de frames RAW pro job ativo da sessão (T059): Snapshot entrega
//! o frame a quem pediu e encerra; Sequência grava em disco e acumula
//! progresso; sem job ativo, frame é descartado sem panicar (corrida rara
//! entre stop e um frame já a caminho).

use camlink_lib::model::{RawJobKind, RawJobState};
use camlink_lib::raw_manager::{
    dng_filename, encode_frame, handle_incoming_frame, parse_frame, stop_sequence, RawFrame,
    RawFrameMetadata, RawJobRuntime,
};

fn sample_frame(seq: u64) -> RawFrame {
    let metadata = RawFrameMetadata {
        seq,
        timestamp_ms: 1_785_800_000_000 + seq,
        width: 4032,
        height: 3024,
    };
    let encoded = encode_frame(&metadata, &[0xCD; 64]);
    parse_frame(&encoded).unwrap().unwrap().0
}

#[test]
fn frame_with_no_active_job_is_dropped_without_panic() {
    let mut slot: Option<RawJobRuntime> = None;
    let result = handle_incoming_frame(&mut slot, sample_frame(1));
    assert!(result.is_none());
    assert!(slot.is_none());
}

#[test]
fn snapshot_delivers_frame_to_waiter_and_clears_slot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let mut slot = Some(RawJobRuntime::snapshot(dir.path().to_path_buf(), tx));

    let frame = sample_frame(7);
    let job = handle_incoming_frame(&mut slot, frame.clone()).expect("job deveria existir");

    assert_eq!(job.kind, RawJobKind::Snapshot);
    assert_eq!(job.state, RawJobState::Done);
    assert!(slot.is_none(), "snapshot é um job de um frame só");

    let received = rx.try_recv().expect("frame deveria ter sido entregue");
    assert_eq!(received, frame);
}

#[test]
fn snapshot_does_not_write_to_disk_itself() {
    // Quem grava o snapshot é o chamador do comando (depois de receber o
    // frame pelo oneshot), não `handle_incoming_frame` — evita gravar duas
    // vezes se o chamador quiser, por exemplo, oferecer um nome diferente.
    let dir = tempfile::tempdir().expect("tempdir");
    let (tx, _rx) = tokio::sync::oneshot::channel();
    let mut slot = Some(RawJobRuntime::snapshot(dir.path().to_path_buf(), tx));

    handle_incoming_frame(&mut slot, sample_frame(1));

    let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert!(
        entries.is_empty(),
        "handle_incoming_frame não deve escrever arquivo no caso Snapshot"
    );
}

#[test]
fn sequence_writes_each_frame_and_accumulates_progress() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut slot = Some(RawJobRuntime::sequence(dir.path().to_path_buf(), 2.0));

    let job1 = handle_incoming_frame(&mut slot, sample_frame(1)).unwrap();
    match job1.state {
        RawJobState::Running { frames, bytes, .. } => {
            assert_eq!(frames, 1);
            assert_eq!(bytes, 64);
        }
        other => panic!("esperava Running, veio {other:?}"),
    }

    let job2 = handle_incoming_frame(&mut slot, sample_frame(2)).unwrap();
    match job2.state {
        RawJobState::Running { frames, bytes, .. } => {
            assert_eq!(frames, 2);
            assert_eq!(bytes, 128);
        }
        other => panic!("esperava Running, veio {other:?}"),
    }

    assert!(slot.is_some(), "sequência continua ativa entre frames");

    let mut written: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    written.sort();
    assert_eq!(written.len(), 2);
}

#[test]
fn sequence_write_failure_marks_job_failed_and_clears_slot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("nao-existe");
    let mut slot = Some(RawJobRuntime::sequence(missing, 2.0));

    let job = handle_incoming_frame(&mut slot, sample_frame(1)).expect("deveria reportar a falha");
    assert!(matches!(job.state, RawJobState::Failed(_)));
    assert!(slot.is_none(), "job com falha de gravação encerra sozinho");
}

#[test]
fn stop_sequence_marks_done_and_clears_slot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut slot = Some(RawJobRuntime::sequence(dir.path().to_path_buf(), 2.0));
    handle_incoming_frame(&mut slot, sample_frame(1));

    let job = stop_sequence(&mut slot).expect("job deveria existir");
    assert_eq!(job.state, RawJobState::Done);
    assert!(slot.is_none());
}

#[test]
fn stop_sequence_with_no_active_job_is_a_noop() {
    let mut slot: Option<RawJobRuntime> = None;
    assert!(stop_sequence(&mut slot).is_none());
}

#[test]
fn dng_filenames_within_a_sequence_never_collide() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut slot = Some(RawJobRuntime::sequence(dir.path().to_path_buf(), 3.0));
    for seq in 1..=5u64 {
        handle_incoming_frame(&mut slot, sample_frame(seq));
    }
    let names: std::collections::HashSet<_> = (1..=5u64)
        .map(|seq| dng_filename(&sample_frame(seq).metadata))
        .collect();
    assert_eq!(names.len(), 5, "cada seq deveria gerar um nome único");
}
