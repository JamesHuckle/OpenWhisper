/** The text to show or log for a thrown value; the fallback when it has none. */
export function errorMessage(error: unknown, fallback = "Transcription failed"): string {
  if (error instanceof Error && error.message.trim()) return error.message;
  const message = String(error ?? "").trim();
  return message || fallback;
}
