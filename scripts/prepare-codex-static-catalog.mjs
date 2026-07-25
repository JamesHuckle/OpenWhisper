#!/usr/bin/env node
"use strict";

/**
 * Build a StaticModelsManager catalog for long-lived `codex exec` runs.
 *
 * Desktop/CLI version skew often writes models_cache.json without fields that
 * the CLI deserializer requires (e.g. supports_reasoning_summaries). That forces
 * a cold refresh child every ~TTL, which times out, leaks pipe fds, and kills
 * long A2A delegations (exit 4294967295 on Windows).
 *
 * Pointing exec at this catalog skips the remote refresh loop.
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { homedir } from "node:os";

const outPath = process.argv[2];
const preferredSlug = process.argv[3] || process.env.CURSOR_CODEX_MODEL || "gpt-5.6-sol";
if (!outPath) {
  console.error("Usage: node prepare-codex-static-catalog.mjs <output.json> [model-slug]");
  process.exit(2);
}

const codexHome = process.env.CODEX_HOME || resolve(homedir(), ".codex");
const cachePath = resolve(codexHome, "models_cache.json");

const FALLBACK_MODEL = {
  slug: "gpt-5.6-sol",
  display_name: "GPT-5.6-Sol",
  description: "Latest frontier agentic coding model.",
  default_reasoning_level: "high",
  supported_reasoning_levels: [
    { effort: "low", description: "Fast responses with lighter reasoning" },
    { effort: "medium", description: "Balances speed and reasoning depth" },
    { effort: "high", description: "Greater reasoning depth for complex problems" },
    { effort: "xhigh", description: "Extra high reasoning depth" }
  ],
  shell_type: "shell_command",
  visibility: "list",
  supported_in_api: true,
  priority: 1,
  availability_nux: null,
  upgrade: null,
  base_instructions: "",
  supports_reasoning_summaries: true,
  default_reasoning_summary: "none",
  support_verbosity: false,
  default_verbosity: null,
  apply_patch_tool_type: "freeform",
  truncation_policy: { mode: "tokens", limit: 10000 },
  supports_parallel_tool_calls: true,
  context_window: 272000,
  max_context_window: 272000,
  effective_context_window_percent: 95,
  experimental_supported_tools: [],
  input_modalities: ["text", "image"]
};

function normalizeModel(raw) {
  const model = { ...FALLBACK_MODEL, ...raw };
  if (model.supports_reasoning_summaries == null) {
    model.supports_reasoning_summaries = true;
  }
  if (model.default_reasoning_summary == null) {
    model.default_reasoning_summary = "none";
  }
  if (!("availability_nux" in model)) model.availability_nux = null;
  if (!("upgrade" in model)) model.upgrade = null;
  if (!("default_verbosity" in model)) model.default_verbosity = null;
  if (!Array.isArray(model.supported_reasoning_levels)) {
    model.supported_reasoning_levels = FALLBACK_MODEL.supported_reasoning_levels;
  }
  if (!Array.isArray(model.experimental_supported_tools)) {
    model.experimental_supported_tools = [];
  }
  if (!model.shell_type) model.shell_type = "shell_command";
  if (!model.visibility) model.visibility = "list";
  if (model.apply_patch_tool_type === "unified") {
    model.apply_patch_tool_type = "freeform";
  }
  return model;
}

function loadModels() {
  if (!existsSync(cachePath)) {
    return [normalizeModel({ ...FALLBACK_MODEL, slug: preferredSlug })];
  }
  try {
    const parsed = JSON.parse(readFileSync(cachePath, "utf8"));
    const models = Array.isArray(parsed?.models) ? parsed.models : [];
    if (!models.length) {
      return [normalizeModel({ ...FALLBACK_MODEL, slug: preferredSlug })];
    }
    const normalized = models.map(normalizeModel);
    if (!normalized.some(m => m.slug === preferredSlug)) {
      normalized.unshift(normalizeModel({ ...FALLBACK_MODEL, slug: preferredSlug }));
    }
    return normalized;
  } catch (error) {
    console.error(`Warning: could not read ${cachePath}: ${error.message}`);
    return [normalizeModel({ ...FALLBACK_MODEL, slug: preferredSlug })];
  }
}

const catalog = { models: loadModels() };
mkdirSync(dirname(resolve(outPath)), { recursive: true });
writeFileSync(resolve(outPath), `${JSON.stringify(catalog, null, 2)}\n`, "utf8");
process.stdout.write(resolve(outPath));
