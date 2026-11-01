// Types for FerroLibri plugins, API version 1. Guide: PLUGINS.md.
// Use as `/// <reference path="ferrolibri-plugin.d.ts" />` or copy into your project.

/** Why `host.fetch` threw; available as the `kind` property of the exception. */
type HostErrorKind =
  | "timeout"
  | "connect"
  | "dns"
  | "tls"
  | "blocked_domain"
  | "too_large"
  | "too_many_requests"
  | "cancelled";

/** What `host.fetch` throws when the request could not be made or finished. */
interface HostError extends Error {
  kind: HostErrorKind;
}

interface HostFetchOptions {
  /** Only GET; anything else throws a TypeError. There is no request body in v1. */
  method?: "GET";
  /** Only Accept, Accept-Language, Content-Type, Referer and Range are sent. */
  headers?: Record<string, string>;
}

interface HostFetchResponse {
  /** Any HTTP status, including 4xx and 5xx. */
  status: number;
  /** The address of the response after redirects: resolve relative links against it. */
  url: string;
  /** Lower-case names; repeated headers are joined with ", "; no set-cookie. */
  headers: Record<string, string>;
  /** UTF-8 text; binary responses are not supported in v1. */
  body: string;
}

interface HtmlNode {
  /** The text of the element. */
  text: string;
  /** The markup of the element, itself included. */
  html: string;
  attrs: Record<string, string>;
}

interface XmlNode {
  /** Local name, without a prefix. */
  name: string;
  /** Namespace URI, or null. */
  ns: string | null;
  /** By local name. */
  attrs: Record<string, string>;
  /** All the text of the element, nested elements included. */
  text: string;
  children: XmlNode[];
}

declare const host: {
  fetch(url: string, options?: HostFetchOptions): HostFetchResponse;
  url: { resolve(base: string, ref: string): string };
  html: { select(html: string, selector: string): HtmlNode[] };
  xml: { parse(text: string): XmlNode };
  log(level: "debug" | "info" | "warn" | "error", message: unknown): void;
};

/** Values of the settings declared in plugin.toml, by key: what the user saved, else the default. */
interface PluginContext {
  settings: Record<string, string | boolean>;
}

interface SearchQuery {
  text: string;
  /** 1-based; may be ignored by plugins that paginate by cursor. */
  page: number;
  /** What `search` returned as `next_cursor` for the previous page; absent on the first page. */
  cursor?: string;
}

/** A downloadable file offered for a result. At most 10 per result. */
interface Offer {
  /** At most 200 characters; unique within the result. */
  id: string;
  format: "EPUB" | "FB2" | "MOBI" | "AZW3" | "PDF" | "TXT" | "DOCX";
  /** Only "zip", and only with format "FB2" (a *.fb2.zip file). */
  archive?: "zip";
  /** Bytes, 1 to 2 GiB. */
  size?: number;
  /** At most 100 characters. */
  label?: string;
}

interface SearchHit {
  /** At most 200 characters; unique within the page. */
  id: string;
  /** At most 300 characters. */
  title: string;
  /** At most 20 names of at most 120 characters. */
  authors?: string[];
  /** At most 8000 characters. */
  description?: string;
  /** BCP 47 tag, e.g. "en" or "pt-BR". */
  language?: string;
  /** 1 to 2100. */
  year?: number;
  /** ISBN-10 or ISBN-13. */
  isbn?: string;
  publisher?: string;
  series?: string;
  /** 0 to 100000. */
  series_index?: number;
  /** At most 30 tags of at most 60 characters. */
  tags?: string[];
  /** http(s); the domain must be allowed. */
  cover_url?: string;
  /** http(s); shown as the "Open page" button. */
  page_url?: string;
  offers?: Offer[];
}

interface SearchPage {
  /** At most 100 hits. A hit that breaks a limit is dropped, the rest are kept. */
  hits: SearchHit[];
  /** Opaque string of at most 500 characters; null or absent on the last page. */
  next_cursor?: string | null;
}

/** Fields to merge into a result. `id` is never overwritten; `offers` replaces the old list. */
type SearchHitDetails = Partial<SearchHit> | null;

interface DownloadSpec {
  /** http(s); the domain must be allowed. */
  url: string;
  /** Overrides the offer's `archive`; only "zip", only for FB2. */
  archive?: "zip";
}

declare function search(query: SearchQuery, ctx: PluginContext): SearchPage;
declare function details(hit: SearchHit, ctx: PluginContext): SearchHitDetails;
declare function resolveDownload(offer: Offer, ctx: PluginContext): DownloadSpec;
