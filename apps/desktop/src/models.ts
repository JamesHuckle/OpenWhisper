// The transcription and tidy models come from the one shared list in
// packages/protocol/models.json, which the Rust app and the worker also read.
import shared from "../../../packages/protocol/models.json";

export type TranscriptionModel = string;

export type TranscriptionModelOption = {
  id: TranscriptionModel;
  /** The text of the model's entry in the settings menu. */
  label: string;
  /** The line shown under the menu when the model is selected. */
  hint: string;
};

export const TRANSCRIPTION_MODELS: readonly TranscriptionModelOption[] =
  shared.transcription_models;
export const DEFAULT_TRANSCRIPTION_MODEL: TranscriptionModel = shared.default_transcription_model;
export const LIVE_TRANSCRIPTION_MODEL: TranscriptionModel = shared.live_transcription_model;
/** The worker's default tidy model, named in the refine settings text. */
export const REFINE_MODEL: string = shared.refine_model;

/** Live transcription streams audio while the user speaks and types as it goes. */
export function isLiveTranscription(model: TranscriptionModel | undefined): boolean {
  return model === LIVE_TRANSCRIPTION_MODEL;
}

export function transcriptionModelHint(model: TranscriptionModel): string {
  return TRANSCRIPTION_MODELS.find((option) => option.id === model)?.hint ?? "";
}

export function transcriptionModelOptionsHtml(): string {
  return TRANSCRIPTION_MODELS.map(
    (option) => `<option value="${escapeHtml(option.id)}">${escapeHtml(option.label)}</option>`,
  ).join("\n        ");
}

function escapeHtml(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}
