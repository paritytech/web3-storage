import { describe, expect, it } from "vitest";
import { parseMultiaddrToHttp } from "@/lib/drive-client";

describe("parseMultiaddrToHttp", () => {
  it("parses /ip4/<host>/tcp/<port>", () => {
    expect(parseMultiaddrToHttp("/ip4/127.0.0.1/tcp/3333")).toBe("http://127.0.0.1:3333");
  });

  it("parses /dns4/<host>/tcp/<port>", () => {
    expect(parseMultiaddrToHttp("/dns4/provider.example.com/tcp/443")).toBe(
      "http://provider.example.com:443",
    );
  });

  it("brackets ip6 hosts", () => {
    expect(parseMultiaddrToHttp("/ip6/::1/tcp/3333")).toBe("http://[::1]:3333");
  });

  it("returns null when port is missing", () => {
    expect(parseMultiaddrToHttp("/ip4/127.0.0.1")).toBeNull();
  });

  it("returns null when host is missing", () => {
    expect(parseMultiaddrToHttp("/tcp/3333")).toBeNull();
  });

  it("ignores trailing protocol layers (e.g. /p2p/<peer>)", () => {
    expect(
      parseMultiaddrToHttp("/ip4/127.0.0.1/tcp/3333/p2p/12D3KooWAbcd"),
    ).toBe("http://127.0.0.1:3333");
  });

  it("uses the first host/port when multiple are present", () => {
    expect(
      parseMultiaddrToHttp("/ip4/10.0.0.1/tcp/4000/ip4/127.0.0.1/tcp/3333"),
    ).toBe("http://10.0.0.1:4000");
  });

  it("returns null on garbage input", () => {
    expect(parseMultiaddrToHttp("not-a-multiaddr")).toBeNull();
    expect(parseMultiaddrToHttp("")).toBeNull();
  });
});
