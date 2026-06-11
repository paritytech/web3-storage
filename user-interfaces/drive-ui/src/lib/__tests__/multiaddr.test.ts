import { describe, expect, it } from "vitest";
import { parseMultiaddrToUrl } from "@web3-storage/sdk";

describe("parseMultiaddrToUrl", () => {
  it("parses /ip4/<host>/tcp/<port> as http", () => {
    expect(parseMultiaddrToUrl("/ip4/127.0.0.1/tcp/3333")).toBe("http://127.0.0.1:3333");
  });

  it("brackets ip6 hosts", () => {
    expect(parseMultiaddrToUrl("/ip6/::1/tcp/3333")).toBe("http://[::1]:3333");
  });

  it("treats a bare /tcp/443 dns host as https", () => {
    expect(parseMultiaddrToUrl("/dns4/provider.example.com/tcp/443")).toBe(
      "https://provider.example.com",
    );
  });

  it("selects https from a /tls/http layer and omits the default :443", () => {
    expect(parseMultiaddrToUrl("/dns4/host.example.com/tcp/443/tls/http")).toBe(
      "https://host.example.com",
    );
  });

  it("keeps a non-default https port", () => {
    expect(parseMultiaddrToUrl("/dns4/host.example.com/tcp/8443/tls/http")).toBe(
      "https://host.example.com:8443",
    );
  });

  it("omits the default http :80", () => {
    expect(parseMultiaddrToUrl("/ip4/127.0.0.1/tcp/80")).toBe("http://127.0.0.1");
  });

  it("appends an http-path reverse-proxy prefix (the hosted-provider form)", () => {
    expect(
      parseMultiaddrToUrl(
        "/dns4/previewnet.substrate.dev/tcp/443/tls/http/http-path/web3-storage-provider",
      ),
    ).toBe("https://previewnet.substrate.dev/web3-storage-provider");
  });

  it("percent-decodes a multi-segment http-path", () => {
    expect(
      parseMultiaddrToUrl("/dns4/host.example.com/tcp/443/tls/http/http-path/a%2fb%2fc"),
    ).toBe("https://host.example.com/a/b/c");
  });

  it("returns null when the multiaddr has no port", () => {
    // multiaddrToUri yields a scheme-less host here, which is not a usable endpoint.
    expect(parseMultiaddrToUrl("/ip4/127.0.0.1")).toBeNull();
  });

  it("returns null for a non-HTTP scheme (bare /tls without /http → tcp://)", () => {
    expect(parseMultiaddrToUrl("/dns4/host.example.com/tcp/443/tls")).toBeNull();
  });

  it("returns null for a /p2p-terminated address (tcp://, not an HTTP endpoint)", () => {
    expect(parseMultiaddrToUrl("/ip4/127.0.0.1/tcp/3333/p2p/12D3KooWAbcd")).toBeNull();
  });

  it("returns null on malformed input", () => {
    expect(parseMultiaddrToUrl("not-a-multiaddr")).toBeNull();
    expect(parseMultiaddrToUrl("/tcp/3333")).toBeNull();
    expect(parseMultiaddrToUrl("")).toBeNull();
  });
});
