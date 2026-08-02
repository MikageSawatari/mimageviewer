import test from "node:test";
import assert from "node:assert/strict";

import {
  resolveVideoPlaylist,
  videoUserErrorMessage,
} from "./video-stream.mjs";

const jsonResponse = (status, body) => new Response(JSON.stringify(body), {
  status,
  headers: { "Content-Type": "application/json" },
});

test("playlist 409 refreshes generation from state and recovers", async () => {
  const requested = [];
  let stateCalls = 0;
  const result = await resolveVideoPlaylist({
    initialUrl: "/stream/2/1/index.m3u8",
    session: 2,
    fetchPlaylist: async (url) => {
      requested.push(url);
      return requested.length === 1
        ? jsonResponse(409, { error: "stream_generation_mismatch" })
        : new Response("#EXTM3U", { status: 200 });
    },
    fetchState: async () => {
      stateCalls += 1;
      return { session: 2, generation: 3 };
    },
    delay: async () => {},
  });

  assert.equal(result.ok, true);
  assert.equal(result.url, "/stream/2/3/index.m3u8");
  assert.deepEqual(requested, [
    "/stream/2/1/index.m3u8",
    "/stream/2/3/index.m3u8",
  ]);
  assert.equal(stateCalls, 1);
});

test("playlist generation recovery is finite and exposes failure", async () => {
  let playlistCalls = 0;
  let stateCalls = 0;
  const result = await resolveVideoPlaylist({
    initialUrl: "/stream/1/1/index.m3u8",
    session: 1,
    maxAttempts: 3,
    timeoutMs: 60000,
    fetchPlaylist: async () => {
      playlistCalls += 1;
      return jsonResponse(409, { error: "stream_generation_mismatch" });
    },
    fetchState: async () => {
      stateCalls += 1;
      return { session: 1, generation: 1 };
    },
    delay: async () => {},
  });

  assert.equal(result.ok, false);
  assert.equal(result.decision.kind, "playlist_recovery_exhausted");
  assert.equal(result.decision.message, "動画の再生を続けられませんでした。");
  assert.equal(playlistCalls, 3);
  assert.equal(stateCalls, 3);
});

test("user video errors hide internal stage details and keep code-based guidance", () => {
  const error = new Error("要求された動画の player が 2 秒以内に準備できませんでした");
  error.code = "stream_start_seek_timeout";
  const message = videoUserErrorMessage(error, "動画を操作できませんでした");

  assert.equal(message, "動画を開始できませんでした。もう一度お試しください。");
  for (const term of ["player", "seek", "2 秒", "予算", "内部状態", "状態が一致"]) {
    assert.equal(message.includes(term), false, term);
  }
  assert.equal(
    videoUserErrorMessage(
      { code: "stream_session_mismatch", message: "動画配信の状態が一致しません" },
      "動画を操作できませんでした"
    ),
    "動画の配信が終了しました。もう一度開いてください。"
  );
});
