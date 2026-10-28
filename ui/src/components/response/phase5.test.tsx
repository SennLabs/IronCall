import { fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setApi, tauriApi } from "../../ipc/api";
import type { ResponseBodyInfo, TlsInfo } from "../../ipc/bindings";
import { createMockApi, type MockApi, mockDraft, mockTimeline } from "../../ipc/mock";
import { previewUrl } from "../../ipc/scheme";
import { HtmlPreview } from "./BodyPreview";
import { ResponseBody } from "./ResponseBody";
import { Timeline } from "./Timeline";
import { TlsSummary } from "./TlsSummary";

const MB = 1024 * 1024;

function bodyOf(mock: MockApi, text: string, over: Partial<ResponseBodyInfo>): ResponseBodyInfo {
  const id = 9000 + mock.bodies.size;
  mock.bodies.set(id, text);
  return {
    bodyId: id,
    kind: "json",
    contentType: "application/json",
    charset: "utf-8",
    charsetLossy: false,
    size: text.length,
    wireSize: text.length,
    rowCount: text.split("\n").length,
    prettyAvailable: false,
    ...over,
  };
}

describe("HTML preview", () => {
  it("loads from the isolated origin with an empty sandbox", () => {
    render(() => <HtmlPreview bodyId={3} />);
    const frame = screen.getByTitle("HTML preview") as HTMLIFrameElement;
    expect(frame.getAttribute("sandbox")).toBe("");
    expect(frame.getAttribute("src")).toBe(previewUrl(3));
    expect(frame.getAttribute("src")).toContain("ironcall-preview");
  });

  it("opting into scripts adds only allow-scripts, never allow-same-origin", () => {
    render(() => <HtmlPreview bodyId={3} />);
    fireEvent.click(screen.getByLabelText("Allow scripts"));
    const frame = screen.getByTitle("HTML preview") as HTMLIFrameElement;
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts");
    expect(frame.getAttribute("sandbox")).not.toContain("allow-same-origin");
  });
});

describe("large bodies", () => {
  afterEach(() => setApi(tauriApi));

  it("pretty-prints JSON up to 10 MB into a derived body by default", async () => {
    const mock = createMockApi();
    setApi(mock);
    const body = bodyOf(mock, '{"a":1,"b":[1,2]}', { size: 3 * MB });
    const { unmount } = render(() => <ResponseBody body={body} status={200} />);
    expect(await screen.findByText(/lines · pretty/)).toBeInTheDocument();
    const pretty = mock.bodyCalls.filter((c) => c.op === "bodyPretty");
    expect(pretty).toHaveLength(1);
    // The derived body is closed when the view goes away.
    const derivedId = [...mock.bodies.keys()].at(-1) as number;
    unmount();
    await waitFor(() => expect(mock.bodies.has(derivedId)).toBe(false));
  });

  it("shows bodies over 10 MB raw, with Pretty-print anyway", async () => {
    const mock = createMockApi();
    setApi(mock);
    const body = bodyOf(mock, '{"a":1}', { size: 12 * MB });
    render(() => <ResponseBody body={body} status={200} />);
    expect(await screen.findByText(/lines · raw/)).toBeInTheDocument();
    expect(mock.bodyCalls.some((c) => c.op === "bodyPretty")).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Pretty-print anyway" }));
    expect(await screen.findByText(/lines · pretty/)).toBeInTheDocument();
  });

  it("saves the response with a suggested file name", async () => {
    const mock = createMockApi();
    setApi(mock);
    const body = bodyOf(mock, '{"a":1}', { size: 7 });
    render(() => <ResponseBody body={body} status={200} url="https://api.test/users/42" />);
    fireEvent.click(await screen.findByRole("button", { name: "Save response…" }));
    await waitFor(() =>
      expect(mock.bodyCalls).toContainEqual({ op: "bodySaveAs", args: [body.bodyId, "42.json"] }),
    );
  });
});

describe("TLS summary", () => {
  const tls: TlsInfo = {
    version: "TLSv1.3",
    cipher: "TLS13_AES_128_GCM_SHA256",
    alpn: "h2",
    certificate: {
      subject: "CN=api.test",
      issuer: "CN=Test CA",
      notBefore: "2026-01-01T00:00:00Z",
      notAfter: "2026-10-20T00:00:00Z",
      sans: ["api.test", "www.api.test"],
      chainLength: 2,
    },
    verified: true,
  };

  it("warns when the certificate expires within 30 days", () => {
    render(() => <TlsSummary tls={tls} now={new Date("2026-10-05T00:00:00Z")} />);
    expect(screen.getByTestId("tls-expiry")).toHaveTextContent("expires in 15 days");
    expect(screen.getByText("www.api.test")).toBeInTheDocument();
  });

  it("has no warning for a healthy certificate, but always flags verification off", () => {
    render(() => (
      <TlsSummary tls={{ ...tls, verified: false }} now={new Date("2026-06-01T00:00:00Z")} />
    ));
    expect(screen.queryByTestId("tls-expiry")).toBeNull();
    expect(screen.getByRole("alert")).toHaveTextContent("verification was turned off");
  });
});

describe("timeline", () => {
  it("renders entries, collapses header blocks and copies as text", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const entries = mockTimeline(mockDraft({ url: "https://api.test/x" }));
    render(() => <Timeline entries={entries} />);
    expect(screen.getByText(/Resolving api.example.com/)).toBeInTheDocument();
    const block = screen.getByRole("button", { name: /GET https:\/\/api.test\/x HTTP\/1.1/ });
    expect(screen.getByText(/user-agent: IronCall/)).toBeInTheDocument();
    fireEvent.click(block);
    expect(block).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText(/user-agent: IronCall/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Copy timeline" }));
    await waitFor(() => expect(writeText).toHaveBeenCalled());
    expect(writeText.mock.calls[0]?.[0]).toContain("> GET https://api.test/x HTTP/1.1");
  });
});
