export function transcriptPreview(transcript: string, maxLength = 120): string {
  const compact = transcript.replace(/\s+/g, " ").trim();
  if (!compact) return "";

  const sentenceEnd = compact.search(/[.!?](?:\s|$)/);
  let preview = sentenceEnd >= 0 ? compact.slice(0, sentenceEnd + 1) : compact;
  if (preview.length > maxLength) {
    const candidate = preview.slice(0, maxLength + 1);
    const wordBoundary = candidate.lastIndexOf(" ");
    preview = candidate.slice(0, wordBoundary >= Math.floor(maxLength * 0.6) ? wordBoundary : maxLength);
  }

  return preview.length < compact.length ? `${preview.trimEnd()}…` : preview;
}
