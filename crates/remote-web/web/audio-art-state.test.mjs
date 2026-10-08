import test from "node:test";
import assert from "node:assert/strict";
import { AudioArtListState, audioArtFailure, normalizedAudioIndicator } from "./audio-art-state.mjs";

const entry = { kind: "audio", path: "C:/Music/song.mp3" };
const addressKey = (value) => value.path.toLowerCase();
const owner = (entries = [entry]) => new AudioArtListState(entries, addressKey, "nonce", "session");

for (const terminal of ["NoArt", "Failed"]) {
  for (const cacheMode of ["Off", "Auto-without-absence", "On"]) {
    test(`${terminal} survives DOM eviction with cache ${cacheMode}`, () => {
      const list = owner();
      assert.equal(list.terminal(entry), undefined);
      assert.equal(list.settle(entry, terminal), true);
      // Remounted cells reference the same adopted owner, with a new entry object.
      assert.equal(list.terminal({ ...entry }), terminal);
      assert.equal(list.terminalByAddress.size, 1);
      assert.equal(owner().terminal(entry), undefined); // explicit refresh
    });
  }
}

test("duplicate bookmarks share terminal source address", () => {
  const list = owner([entry, { ...entry, bookmarkId: "second" }]);
  list.settle(entry, "NoArt");
  assert.equal(list.addresses.size, 1);
  assert.equal(list.terminal({ ...entry, bookmarkId: "second" }), "NoArt");
});

test("terminal retention is bounded by unique available audio addresses", () => {
  const list = owner([entry, {kind:"image",path:"x"}, {kind:"audio",path:"missing",unavailable:true}]);
  assert.equal(list.settle({kind:"audio",path:"outside"}, "Failed"), false);
  assert.equal(list.settle(entry, "Loaded"), false);
  assert.equal(list.addresses.size, 1);
  assert.equal(list.terminalByAddress.size, 0);
});

for (const [status, error, expected] of [
  [422,"no_thumbnail","NoArt"], [422,"miv_thumbnail_error","Failed"],
  [422,undefined,"Failed"], [404,"no_thumbnail","Failed"], [0,"network_error","Failed"],
  [401,"unauthorized",null], [409,"session_required",null], [428,"session_required",null],
]) test(`error classification ${status}/${error}`, () => assert.equal(audioArtFailure(status,error), expected));

for (const [value,expected] of [[undefined,"music_note_icon"],["unknown","music_note_icon"],["music_note_icon","music_note_icon"],["bottom_left_badge","bottom_left_badge"],["hidden","hidden"]]) {
  test(`indicator defensive default ${value}`, () => assert.equal(normalizedAudioIndicator(value),expected));
}
