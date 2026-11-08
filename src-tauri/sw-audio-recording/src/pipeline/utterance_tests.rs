use super::super::{DEFAULT_INTERIM_SECS, SAMPLE_RATE};
use super::*;
use crate::vad::VAD_CHUNK_SAMPLES;

const PREROLL: usize = 3200;
const PIECE_SAMPLES: usize = (DEFAULT_INTERIM_SECS * SAMPLE_RATE) as usize;
const CHUNKS_PER_PIECE: usize = PIECE_SAMPLES.div_ceil(VAD_CHUNK_SAMPLES);

#[derive(Default)]
struct LengthDecoder {
    decoded_lengths: Vec<usize>,
    empty_lengths: Vec<usize>,
    failing_lengths: Vec<usize>,
}

impl LengthDecoder {
    fn empty_for(lengths: &[usize]) -> Self {
        Self {
            empty_lengths: lengths.to_vec(),
            ..Self::default()
        }
    }

    fn failing_for(lengths: &[usize]) -> Self {
        Self {
            failing_lengths: lengths.to_vec(),
            ..Self::default()
        }
    }

    fn decode(&mut self, samples: &[f32]) -> Option<String> {
        self.decoded_lengths.push(samples.len());
        if self.failing_lengths.contains(&samples.len()) {
            None
        } else if self.empty_lengths.contains(&samples.len()) {
            Some(String::new())
        } else {
            Some(label(samples.len()))
        }
    }
}

fn label(length: usize) -> String {
    format!("<{length}>")
}

fn chunks(count: usize) -> usize {
    count * VAD_CHUNK_SAMPLES
}

fn started() -> UtteranceAssembler {
    let mut assembler = UtteranceAssembler::new(PIECE_SAMPLES);
    assembler.begin(&vec![0.0; PREROLL]);
    assembler
}

fn push_chunks<F>(
    assembler: &mut UtteranceAssembler,
    count: usize,
    transcribe: &mut F,
) -> Vec<PipelineEvent>
where
    F: FnMut(&[f32]) -> Option<String>,
{
    let chunk = [0.0f32; VAD_CHUNK_SAMPLES];
    (0..count)
        .filter_map(|_| assembler.push_speech(&chunk, transcribe))
        .collect()
}

fn texts(events: &[PipelineEvent], wanted_final: bool) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            PipelineEvent::Transcript { text, is_final } if *is_final == wanted_final => {
                Some(text.clone())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn final_text_is_one_decode_of_all_utterance_audio_not_the_join_of_pieces() {
    let mut decoder = LengthDecoder::default();
    let mut transcribe = |samples: &[f32]| decoder.decode(samples);
    let mut assembler = started();

    let mut events = push_chunks(&mut assembler, CHUNKS_PER_PIECE + 20, &mut transcribe);
    events.extend(assembler.finish(&mut transcribe));

    let first_piece = PREROLL + chunks(CHUNKS_PER_PIECE);
    let tail_piece = chunks(20);
    let whole_utterance = PREROLL + chunks(CHUNKS_PER_PIECE + 20);
    assert_eq!(
        texts(&events, false),
        vec![label(first_piece), label(tail_piece)]
    );
    assert_eq!(texts(&events, true), vec![label(whole_utterance)]);
    assert_eq!(
        decoder.decoded_lengths,
        vec![first_piece, tail_piece, whole_utterance]
    );
}

#[test]
fn live_pieces_are_still_emitted_as_non_final_every_five_seconds() {
    let mut decoder = LengthDecoder::default();
    let mut transcribe = |samples: &[f32]| decoder.decode(samples);
    let mut assembler = started();

    let before_first_piece = push_chunks(&mut assembler, CHUNKS_PER_PIECE - 1, &mut transcribe);
    let first_piece = push_chunks(&mut assembler, 1, &mut transcribe);
    let second_piece = push_chunks(&mut assembler, CHUNKS_PER_PIECE, &mut transcribe);

    assert!(before_first_piece.is_empty());
    assert_eq!(
        texts(&first_piece, false),
        vec![label(PREROLL + chunks(CHUNKS_PER_PIECE))]
    );
    assert_eq!(
        texts(&second_piece, false),
        vec![label(chunks(CHUNKS_PER_PIECE))]
    );
    assert!(texts(&first_piece, true).is_empty());
    assert!(texts(&second_piece, true).is_empty());
}

#[test]
fn force_segment_closes_an_ordered_chunk_and_restarts_the_accumulator() {
    let mut decoder = LengthDecoder::default();
    let mut transcribe = |samples: &[f32]| decoder.decode(samples);
    let mut assembler = started();
    let boundary_chunk = [0.0f32; VAD_CHUNK_SAMPLES];

    let mut events = push_chunks(&mut assembler, 200, &mut transcribe);
    events.extend(assembler.force_boundary(&boundary_chunk, &mut transcribe));
    events.extend(push_chunks(&mut assembler, 10, &mut transcribe));
    events.extend(assembler.finish(&mut transcribe));

    let first_chunk = PREROLL + chunks(200);
    let second_chunk = chunks(11);
    assert_eq!(
        texts(&events, true),
        vec![format!("{} {}", label(first_chunk), label(second_chunk))]
    );
    assert_eq!(
        decoder.decoded_lengths,
        vec![
            PREROLL + chunks(CHUNKS_PER_PIECE),
            chunks(200 - CHUNKS_PER_PIECE),
            first_chunk,
            second_chunk,
        ]
    );
}

#[test]
fn audio_after_a_force_segment_counts_the_boundary_chunk_toward_the_next_piece() {
    let mut decoder = LengthDecoder::default();
    let mut transcribe = |samples: &[f32]| decoder.decode(samples);
    let mut assembler = started();
    let boundary_chunk = [0.0f32; VAD_CHUNK_SAMPLES];

    push_chunks(&mut assembler, 10, &mut transcribe);
    assembler.force_boundary(&boundary_chunk, &mut transcribe);
    let before_piece = push_chunks(&mut assembler, CHUNKS_PER_PIECE - 2, &mut transcribe);
    let piece = push_chunks(&mut assembler, 1, &mut transcribe);

    assert!(before_piece.is_empty());
    assert_eq!(texts(&piece, false), vec![label(chunks(CHUNKS_PER_PIECE))]);
}

#[test]
fn empty_whole_utterance_decode_falls_back_to_the_joined_pieces() {
    let whole_utterance = PREROLL + chunks(CHUNKS_PER_PIECE + 20);
    let mut decoder = LengthDecoder::empty_for(&[whole_utterance]);
    let mut transcribe = |samples: &[f32]| decoder.decode(samples);
    let mut assembler = started();

    let mut events = push_chunks(&mut assembler, CHUNKS_PER_PIECE + 20, &mut transcribe);
    events.extend(assembler.finish(&mut transcribe));

    let pieces = format!(
        "{} {}",
        label(PREROLL + chunks(CHUNKS_PER_PIECE)),
        label(chunks(20))
    );
    assert_eq!(texts(&events, true), vec![pieces]);
}

#[test]
fn fallback_to_pieces_applies_only_to_the_forced_chunk_whose_whole_decode_was_empty() {
    let first_chunk = PREROLL + chunks(200);
    let mut decoder = LengthDecoder::failing_for(&[first_chunk]);
    let mut transcribe = |samples: &[f32]| decoder.decode(samples);
    let mut assembler = started();
    let boundary_chunk = [0.0f32; VAD_CHUNK_SAMPLES];

    push_chunks(&mut assembler, 200, &mut transcribe);
    assembler.force_boundary(&boundary_chunk, &mut transcribe);
    push_chunks(&mut assembler, 10, &mut transcribe);
    let events = assembler.finish(&mut transcribe);

    let expected = format!(
        "{} {} {}",
        label(PREROLL + chunks(CHUNKS_PER_PIECE)),
        label(chunks(200 - CHUNKS_PER_PIECE)),
        label(chunks(11))
    );
    assert_eq!(texts(&events, true), vec![expected]);
}

#[test]
fn empty_whole_decode_without_any_piece_text_emits_no_final() {
    let mut decoder = LengthDecoder::failing_for(&[PREROLL + chunks(30)]);
    let mut transcribe = |samples: &[f32]| decoder.decode(samples);
    let mut assembler = started();

    push_chunks(&mut assembler, 30, &mut transcribe);
    let events = assembler.finish(&mut transcribe);

    assert!(events.is_empty());
}

#[test]
fn utterance_shorter_than_one_piece_is_decoded_once_for_both_live_and_final_text() {
    let mut decoder = LengthDecoder::default();
    let mut transcribe = |samples: &[f32]| decoder.decode(samples);
    let mut assembler = started();

    push_chunks(&mut assembler, 30, &mut transcribe);
    let events = assembler.finish(&mut transcribe);

    let whole_utterance = PREROLL + chunks(30);
    assert_eq!(texts(&events, false), vec![label(whole_utterance)]);
    assert_eq!(texts(&events, true), vec![label(whole_utterance)]);
    assert_eq!(decoder.decoded_lengths, vec![whole_utterance]);
}

#[test]
fn audio_below_the_minimum_utterance_length_is_never_decoded() {
    let mut decoder = LengthDecoder::default();
    let mut transcribe = |samples: &[f32]| decoder.decode(samples);
    let mut assembler = UtteranceAssembler::new(PIECE_SAMPLES);
    assembler.begin(&[]);

    push_chunks(
        &mut assembler,
        MIN_UTTERANCE_SAMPLES / VAD_CHUNK_SAMPLES,
        &mut transcribe,
    );
    let events = assembler.finish(&mut transcribe);

    assert!(events.is_empty());
    assert!(decoder.decoded_lengths.is_empty());
}

#[test]
fn clearing_drops_audio_pieces_and_finished_chunks() {
    let mut decoder = LengthDecoder::default();
    let mut transcribe = |samples: &[f32]| decoder.decode(samples);
    let mut assembler = started();
    let boundary_chunk = [0.0f32; VAD_CHUNK_SAMPLES];

    push_chunks(&mut assembler, 200, &mut transcribe);
    assembler.force_boundary(&boundary_chunk, &mut transcribe);
    push_chunks(&mut assembler, 10, &mut transcribe);
    assembler.clear();
    let after_clear = assembler.finish(&mut transcribe);
    assembler.begin(&vec![0.0; PREROLL]);
    push_chunks(&mut assembler, 10, &mut transcribe);
    let next_utterance = assembler.finish(&mut transcribe);

    assert!(after_clear.is_empty());
    assert_eq!(
        texts(&next_utterance, true),
        vec![label(PREROLL + chunks(10))]
    );
}

#[test]
fn starting_a_new_utterance_discards_the_unfinished_previous_one() {
    let mut decoder = LengthDecoder::default();
    let mut transcribe = |samples: &[f32]| decoder.decode(samples);
    let mut assembler = started();
    let boundary_chunk = [0.0f32; VAD_CHUNK_SAMPLES];

    push_chunks(&mut assembler, 200, &mut transcribe);
    assembler.force_boundary(&boundary_chunk, &mut transcribe);
    assembler.begin(&vec![0.0; PREROLL]);
    push_chunks(&mut assembler, 10, &mut transcribe);
    let events = assembler.finish(&mut transcribe);

    assert_eq!(texts(&events, true), vec![label(PREROLL + chunks(10))]);
}

#[test]
fn flushing_mid_utterance_finalizes_from_all_accumulated_audio() {
    let mut decoder = LengthDecoder::default();
    let mut transcribe = |samples: &[f32]| decoder.decode(samples);
    let mut assembler = started();

    push_chunks(&mut assembler, CHUNKS_PER_PIECE + 13, &mut transcribe);
    let flushed = assembler.finish(&mut transcribe);

    assert_eq!(
        texts(&flushed, true),
        vec![label(PREROLL + chunks(CHUNKS_PER_PIECE + 13))]
    );
}

#[test]
fn flushing_after_speech_end_emits_nothing_and_decodes_nothing_more() {
    let mut decoder = LengthDecoder::default();
    let mut transcribe = |samples: &[f32]| decoder.decode(samples);
    let mut assembler = started();

    push_chunks(&mut assembler, CHUNKS_PER_PIECE + 13, &mut transcribe);
    assembler.finish(&mut transcribe);
    let flushed = assembler.finish(&mut transcribe);

    assert!(flushed.is_empty());
    assert_eq!(decoder.decoded_lengths.len(), 3);
}
