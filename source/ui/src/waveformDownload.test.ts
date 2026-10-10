import { test } from "node:test";
import assert from "node:assert/strict";
import { downloadWaveform } from "./waveformDownload";
const analysis = { detail: null, preview: null, track: null };

test("a slow waveform transfer survives the former five-second deadline", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const controller = new AbortController();
  let complete!: (response: Response) => void;
  const request: typeof fetch = async (_url, options) => {
    assert.equal(options?.signal, controller.signal);
    assert.equal(options?.cache, "no-store");
    return new Promise((resolve) => {
      complete = resolve;
    });
  };
  const download = downloadWaveform(1, "current", controller, request);
  t.mock.timers.tick(6000);
  assert.equal(controller.signal.aborted, false);
  complete(Response.json({ key: "current", analysis }));
  assert.deepEqual(await download, analysis);
});

test("obsolete transfers and changed track responses never supply waveforms", async () => {
  const controller = new AbortController();
  const request: typeof fetch = async () => {
    controller.abort();
    return Response.json({ key: "old", analysis });
  };
  assert.equal(
    await downloadWaveform(1, "old", controller, request),
    undefined,
  );
  assert.equal(
    await downloadWaveform(1, "new", new AbortController(), async () =>
      Response.json({ key: "old", analysis }),
    ),
    undefined,
  );
});

test("a hung waveform transfer is still bounded", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const controller = new AbortController();
  const request: typeof fetch = async () =>
    new Promise((_resolve, reject) => {
      controller.signal.addEventListener("abort", () =>
        reject(new Error("aborted")),
      );
    });
  const download = downloadWaveform(1, "current", controller, request);
  t.mock.timers.tick(15000);
  await assert.rejects(download, /aborted/);
});

test("ongoing download progress survives thirty seconds without being restarted", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const controller = new AbortController();
  let stream!: ReadableStreamDefaultController<Uint8Array>;
  const body = new ReadableStream<Uint8Array>({
    start(value) {
      stream = value;
    },
  });
  const download = downloadWaveform(
    1,
    "current",
    controller,
    async () => new Response(body),
  );
  const chunks = [
    '{"key":"current",',
    '"analysis":',
    JSON.stringify(analysis),
    "}",
  ];
  for (const chunk of chunks) {
    await new Promise<void>((resolve) => setImmediate(resolve));
    t.mock.timers.tick(10000);
    assert.equal(controller.signal.aborted, false);
    stream.enqueue(new TextEncoder().encode(chunk));
  }
  stream.close();
  assert.deepEqual(await download, analysis);
});

test("packed native waveform reconstructs every band and raw byte", async () => {
  const result = await downloadWaveform(
    1,
    "current",
    new AbortController(),
    async (url) => {
      assert.equal(url, "/api/live/analysis/1?waveform=packed");
      return Response.json({
        key: "current",
        analysis: {
          detail: {
            tag: "PWV7",
            packedColumns: "00107f0c2238",
            normalization: 127,
            samplesPerSecond: 150,
          },
          preview: {
            tag: "PWV6",
            packedColumns: "0a141e",
            normalization: 255,
            samplesPerSecond: null,
          },
          track: null,
        },
      });
    },
  );
  assert.deepEqual(result!.detail!.rawColumns, [
    [0, 16, 127],
    [12, 34, 56],
  ]);
  assert.deepEqual(result!.detail!.columns[0], {
    low: 0,
    mid: Math.fround(16 / 127),
    high: 1,
  });
  assert.deepEqual(result!.preview!.columns[0], {
    low: Math.fround(30 / 255),
    mid: Math.fround(10 / 255),
    high: Math.fround(20 / 255),
  });
});

test("malformed packed waveform is rejected rather than displayed incorrectly", async () => {
  await assert.rejects(
    downloadWaveform(1, "current", new AbortController(), async () =>
      Response.json({
        key: "current",
        analysis: {
          detail: { tag: "PWV7", packedColumns: "zz", normalization: 127 },
          preview: null,
        },
      }),
    ),
    /Invalid packed/,
  );
});
