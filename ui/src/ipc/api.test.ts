import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("./bindings", () => ({
  commands: {
    previewRequest: vi.fn(),
    getBodyText: vi.fn(),
    newScratchRequest: vi.fn(),
  },
  events: { sendProgress: { listen: vi.fn() } },
}));

const { commands } = await import("./bindings");
const { api, fromWire, SendFailure, sendError, setApi, tauriApi, toSearchOutcome, toWire } =
  await import("./api");
const { createMockApi, mockDraft } = await import("./mock");

describe("tauriApi", () => {
  afterEach(() => setApi(tauriApi));

  it("unwraps ok results", async () => {
    vi.mocked(commands.getBodyText).mockResolvedValue({ status: "ok", data: "hello" });
    await expect(tauriApi.getBodyText(1, false)).resolves.toBe("hello");
  });

  it("throws SendFailure with the structured error", async () => {
    const error = sendError("invalidUrl", "bad");
    vi.mocked(commands.previewRequest).mockResolvedValue({ status: "error", error });
    const p = tauriApi.previewRequest(mockDraft());
    await expect(p).rejects.toBeInstanceOf(SendFailure);
    await expect(p).rejects.toMatchObject({ error });
  });

  it("throws plain errors for string errors", async () => {
    vi.mocked(commands.getBodyText).mockResolvedValue({ status: "error", error: "unknown body" });
    await expect(tauriApi.getBodyText(1, false)).rejects.toThrow("unknown body");
  });

  it("setApi swaps the implementation behind `api`", async () => {
    const mock = createMockApi();
    setApi(mock);
    const draft = await api.newScratchRequest();
    expect(draft.method).toBe("GET");
    expect(commands.newScratchRequest).not.toHaveBeenCalled();
  });
});

describe("toWire / fromWire", () => {
  it("passes the object through, keeping unknown keys", () => {
    const draft = { ...mockDraft(), futureField: { a: 1 } };
    const wire = toWire(draft);
    expect(wire).toBe(draft);
    expect((wire as Record<string, unknown>).futureField).toEqual({ a: 1 });
    expect(fromWire(wire as never)).toBe(draft);
  });
});

describe("toSearchOutcome", () => {
  it("maps found, notFound and cancelled", () => {
    const hit = { row: 1, offset: 10, column: 2 };
    expect(toSearchOutcome({ outcome: "found", hit })).toEqual({ hit, cancelled: false });
    expect(toSearchOutcome({ outcome: "notFound" })).toEqual({ hit: null, cancelled: false });
    expect(toSearchOutcome({ outcome: "cancelled" })).toEqual({ hit: null, cancelled: true });
  });
});
