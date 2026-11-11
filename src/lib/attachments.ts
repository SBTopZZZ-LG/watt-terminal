// Attachments on an agent message. The input bar takes files from the paperclip, a paste or a drop, and each one becomes an
// AttachmentInput (types.ts), which agent_message takes. A file is classified by its bytes, never by File.type: that is often
// empty (.md, .log) or wrong (.ts reports video/mp2t). An image is always re-encoded here, so the model gets PNG or JPEG at
// most 1568 px on the long edge, with the orientation applied, and without EXIF or GPS, an animated GIF's first frame only, and
// WebP, BMP, TIFF and HEIC converted. The limits match src-tauri/src/core/chat/attachments.rs, which checks them again.

import type { AttachmentInput } from './types';
import type { UserAttachment } from './transcript';
import { visibleEscapes } from './visibleEscapes';

export const LIMITS = {
  /** Attachments on one message. */
  attachments: 8,
  images: 6,
  /** The images of one message, after the re-encode. */
  imageBytes: 5 * 1024 * 1024,
  /** One processed image. A JPEG still above this after a retry is refused. */
  processedImageBytes: 1.5 * 1024 * 1024,
  /** A PNG is kept only at or under this size; a bigger one becomes a flattened JPEG. */
  pngBytes: 1024 * 1024,
  docBytes: 100 * 1024,
  docsBytes: 200 * 1024,
  /** Raw file sizes, checked before the file is read. PNG, JPEG, GIF and WebP have their size read from the header first. */
  rawSniffableBytes: 25 * 1024 * 1024,
  rawOtherImageBytes: 10 * 1024 * 1024,
  megapixels: 50_000_000,
  longEdge: 1568,
  /** The retry after a JPEG that is still too big. */
  retryEdge: 1280,
  thumbEdge: 96,
  thumbChars: 16 * 1024,
} as const;

/** The colour an image is flattened onto when it loses its alpha (the JPEG branch and the thumbnail). */
const BACKGROUND = '#0b1620';

export type ImageFormat = 'png' | 'jpeg' | 'gif' | 'webp' | 'bmp' | 'tiff' | 'heic';

/** The formats whose header gives the size before any decode. The others (BMP, TIFF, HEIC) are decoded by the webview only. */
const SNIFFABLE: ReadonlySet<ImageFormat> = new Set<ImageFormat>(['png', 'jpeg', 'gif', 'webp']);
const HEIC_BRANDS = ['heic', 'heix', 'heif', 'mif1', 'msf1'];

/** How many leading bytes classify a file. */
export const HEAD_BYTES = 64;

interface Dims {
  w: number;
  h: number;
}

/** Whether the bytes at `at` spell `text` (ASCII or Latin-1 code units). */
function spells(bytes: Uint8Array, at: number, text: string): boolean {
  if (at + text.length > bytes.length) {
    return false;
  }
  for (let i = 0; i < text.length; i += 1) {
    if (bytes[at + i] !== text.charCodeAt(i)) {
      return false;
    }
  }
  return true;
}

function le16(b: Uint8Array, at: number): number {
  return b[at] | (b[at + 1] << 8);
}

function le24(b: Uint8Array, at: number): number {
  return b[at] | (b[at + 1] << 8) | (b[at + 2] << 16);
}

function le32(b: Uint8Array, at: number): number {
  return (b[at] | (b[at + 1] << 8) | (b[at + 2] << 16) | (b[at + 3] << 24)) >>> 0;
}

function be16(b: Uint8Array, at: number): number {
  return (b[at] << 8) | b[at + 1];
}

function be32(b: Uint8Array, at: number): number {
  return ((b[at] << 24) | (b[at + 1] << 16) | (b[at + 2] << 8) | b[at + 3]) >>> 0;
}

/**
 * The image format of a file, from its first bytes (`HEAD_BYTES` of them are enough), or null when it is not an image this
 * app knows. A text file never matches: BMP needs NUL bytes in its header, and the others start with bytes text does not have.
 */
export function imageFormatOf(head: Uint8Array): ImageFormat | null {
  if (spells(head, 0, '\x89PNG\r\n\x1a\n')) {
    return 'png';
  }
  if (spells(head, 0, '\xff\xd8\xff')) {
    return 'jpeg';
  }
  if (spells(head, 0, 'GIF87a') || spells(head, 0, 'GIF89a')) {
    return 'gif';
  }
  if (spells(head, 0, 'RIFF') && spells(head, 8, 'WEBP')) {
    return 'webp';
  }
  if (spells(head, 0, 'II*\0') || spells(head, 0, 'MM\0*')) {
    return 'tiff';
  }
  if (spells(head, 4, 'ftyp') && HEIC_BRANDS.some((brand) => spells(head, 8, brand))) {
    return 'heic';
  }
  // BMP: "BM", a reserved word of zero, and a pixel offset past the 14-byte file header.
  if (spells(head, 0, 'BM') && head.length >= 14 && le32(head, 6) === 0 && le32(head, 10) >= 14) {
    return 'bmp';
  }
  return null;
}

/** Whether the file is a PDF (its header says so). PDFs are refused with their own message. */
export function isPdf(head: Uint8Array): boolean {
  return spells(head, 0, '%PDF-');
}

/**
 * The width and height from the header, or null when the header does not give them. The stored size: an EXIF rotation is not
 * applied here, which is fine for the pixel-count gate. WebP has three layouts: VP8X (extended), VP8 (lossy), VP8L (lossless).
 */
export function sniffDims(b: Uint8Array, format: ImageFormat): Dims | null {
  switch (format) {
    case 'png':
      return b.length >= 24 && spells(b, 12, 'IHDR') ? { w: be32(b, 16), h: be32(b, 20) } : null;
    case 'gif':
      return b.length >= 10 ? { w: le16(b, 6), h: le16(b, 8) } : null;
    case 'webp':
      return webpDims(b);
    case 'jpeg':
      return jpegDims(b);
    default:
      return null;
  }
}

function webpDims(b: Uint8Array): Dims | null {
  if (b.length < 30) {
    return null;
  }
  // VP8X: the canvas size, minus one, as 24-bit little-endian numbers after the flags.
  if (spells(b, 12, 'VP8X')) {
    return { w: 1 + le24(b, 24), h: 1 + le24(b, 27) };
  }
  // VP8 (lossy): a key frame start code, then 14-bit width and height (the top two bits are a scale).
  if (spells(b, 12, 'VP8 ')) {
    if (b[23] !== 0x9d || b[24] !== 0x01 || b[25] !== 0x2a) {
      return null;
    }
    return { w: le16(b, 26) & 0x3fff, h: le16(b, 28) & 0x3fff };
  }
  // VP8L (lossless): a signature byte, then width-1 and height-1 as 14-bit fields packed little-endian.
  if (spells(b, 12, 'VP8L')) {
    if (b[20] !== 0x2f) {
      return null;
    }
    const b0 = b[21];
    const b1 = b[22];
    const b2 = b[23];
    const b3 = b[24];
    return { w: 1 + (b0 | ((b1 & 0x3f) << 8)), h: 1 + ((b1 >> 6) | (b2 << 2) | ((b3 & 0x0f) << 10)) };
  }
  return null;
}

/** The first start-of-frame marker's size. Other segments are skipped by their length. */
function jpegDims(b: Uint8Array): Dims | null {
  let i = 2;
  while (i + 4 <= b.length) {
    if (b[i] !== 0xff) {
      return null;
    }
    const marker = b[i + 1];
    if (marker === 0xff) {
      // A fill byte: the next byte is the marker.
      i += 1;
      continue;
    }
    // Standalone markers: SOI, TEM, and the restart markers. They have no length.
    if (marker === 0xd8 || marker === 0x01 || (marker >= 0xd0 && marker <= 0xd7)) {
      i += 2;
      continue;
    }
    if (marker === 0xd9) {
      return null;
    }
    // Start of frame: C0-CF, except C4 (DHT), C8 (JPG) and CC (DAC).
    const sof = marker >= 0xc0 && marker <= 0xcf && marker !== 0xc4 && marker !== 0xc8 && marker !== 0xcc;
    if (sof) {
      if (i + 9 > b.length) {
        return null;
      }
      return { w: be16(b, i + 7), h: be16(b, i + 5) };
    }
    i += 2 + be16(b, i + 2);
  }
  return null;
}

/**
 * Best-effort truncation check on the raw bytes. A JPEG must end with its EOI marker (trailing zero bytes are allowed), a PNG
 * must end with an IEND chunk (a few trailing bytes are tolerated). A truncated JPEG decodes to a garbage image without an
 * error in both engines, so the decode cannot be the check. Other formats are not checked here.
 */
export function looksTruncated(b: Uint8Array, format: ImageFormat): boolean {
  if (format === 'jpeg') {
    let end = b.length;
    while (end > 0 && b[end - 1] === 0) {
      end -= 1;
    }
    return !(end >= 2 && b[end - 2] === 0xff && b[end - 1] === 0xd9);
  }
  if (format === 'png') {
    // The IEND type, then its four CRC bytes. Searched in the last 64 bytes, from the end.
    for (let i = b.length - 4; i >= Math.max(0, b.length - 64); i -= 1) {
      if (spells(b, i, 'IEND')) {
        return i + 8 > b.length;
      }
    }
    return true;
  }
  return false;
}

/** Whether the bytes are UTF-8 text: no NUL byte, and a strict UTF-8 decode. */
export function isTextBytes(b: Uint8Array): boolean {
  if (b.includes(0)) {
    return false;
  }
  try {
    new TextDecoder('utf-8', { fatal: true }).decode(b);
    return true;
  } catch {
    return false;
  }
}

/** The width and height of a box scaled so its long edge is at most `edge`. Never upscaled, never below 1. */
export function fitWithin(width: number, height: number, edge: number): Dims {
  const scale = Math.min(1, edge / Math.max(width, height));
  return { w: Math.max(1, Math.round(width * scale)), h: Math.max(1, Math.round(height * scale)) };
}

/** The decoded size of a base64 string, in bytes. */
export function decodedSize(b64: string): number {
  const padding = b64.endsWith('==') ? 2 : b64.endsWith('=') ? 1 : 0;
  return Math.floor((b64.length * 3) / 4) - padding;
}

/** Base64 of the bytes. Chunked, so a large array is not passed to one call. */
export function bytesToBase64(bytes: Uint8Array): string {
  let binary = '';
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

/** A size for a person: "512 B", "12 KB", "1.5 MB". */
export function formatSize(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${Math.round(bytes / 1024)} KB`;
  }
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

// ---- per-message accounting: what the message still allows. The checks are made here, before the send, and again in Rust.

/** What the attachments already on a message add up to. `bytes` of an image is its processed size. */
export interface Load {
  attachments: number;
  images: number;
  imageBytes: number;
  docBytes: number;
}

export const NO_LOAD: Load = { attachments: 0, images: 0, imageBytes: 0, docBytes: 0 };

/** One attachment: its kind, and its size (processed for an image, the file's for a document). */
export interface Sized {
  kind: 'image' | 'text';
  bytes: number;
}

export function addLoad(load: Load, item: Sized): Load {
  if (item.kind === 'image') {
    return { ...load, attachments: load.attachments + 1, images: load.images + 1, imageBytes: load.imageBytes + item.bytes };
  }
  return { ...load, attachments: load.attachments + 1, docBytes: load.docBytes + item.bytes };
}

export function loadOf(items: Sized[]): Load {
  return items.reduce(addLoad, NO_LOAD);
}

/** The message for one more attachment when `count` are already on the message, or null when there is room. */
export function countRefusal(count: number): string | null {
  return count >= LIMITS.attachments ? `At most ${LIMITS.attachments} attachments per message.` : null;
}

/** The message for one more attachment, or null when it fits the message. */
export function admitRefusal(load: Load, next: Sized): string | null {
  const full = countRefusal(load.attachments);
  if (full !== null) {
    return full;
  }
  if (next.kind === 'image') {
    if (load.images >= LIMITS.images) {
      return `At most ${LIMITS.images} images per message.`;
    }
    if (load.imageBytes + next.bytes > LIMITS.imageBytes) {
      return 'The images of a message total more than 5 MB.';
    }
    return null;
  }
  if (load.docBytes + next.bytes > LIMITS.docsBytes) {
    return 'The text documents of a message total more than 200 KB.';
  }
  return null;
}

// ---- files. The browser side: decode, re-encode, thumbnail. Only what the canvas needs runs here.

/** A refusal for one file. Its message is the user's, and is shown with the file's name in front. */
class Refused extends Error {}

/** The result of checking one file: the attachment, or the message that says why it cannot be attached. */
export type Prepared = { ok: true; input: AttachmentInput; bytes: number } | { ok: false; message: string };

/** Checks one file and turns it into an attachment. Never throws: a failure is a message. */
export async function prepareFile(file: File): Promise<Prepared> {
  const label = visibleEscapes(file.name || 'file');
  try {
    const { input, bytes } = await convert(file);
    return { ok: true, input, bytes };
  } catch (e) {
    if (e instanceof Refused) {
      return { ok: false, message: `${label}: ${e.message}` };
    }
    return { ok: false, message: `${label}: this file can't be read here` };
  }
}

async function convert(file: File): Promise<{ input: AttachmentInput; bytes: number }> {
  if (file.size === 0) {
    throw new Refused('the file is empty');
  }
  const head = new Uint8Array(await file.slice(0, HEAD_BYTES).arrayBuffer());
  const format = imageFormatOf(head);
  if (format !== null) {
    return convertImage(file, format);
  }
  if (isPdf(head)) {
    throw new Refused("PDFs aren't supported yet");
  }
  if (file.size > LIMITS.docBytes) {
    throw new Refused('larger than 100 KB (text documents are limited to 100 KB each)');
  }
  const raw = new Uint8Array(await file.arrayBuffer());
  if (!isTextBytes(raw)) {
    throw new Refused("this file type can't be attached (only images and UTF-8 text files)");
  }
  return { input: { name: file.name, mime: 'text/plain', kind: 'text', data_b64: bytesToBase64(raw) }, bytes: raw.length };
}

async function convertImage(file: File, format: ImageFormat): Promise<{ input: AttachmentInput; bytes: number }> {
  const sniffable = SNIFFABLE.has(format);
  if (file.size > (sniffable ? LIMITS.rawSniffableBytes : LIMITS.rawOtherImageBytes)) {
    throw new Refused(sniffable ? 'larger than 25 MB' : 'larger than 10 MB');
  }
  if (sniffable) {
    const raw = new Uint8Array(await file.arrayBuffer());
    if (looksTruncated(raw, format)) {
      throw new Refused('the file looks truncated');
    }
    const dims = sniffDims(raw, format);
    if (dims !== null && dims.w * dims.h > LIMITS.megapixels) {
      throw new Refused('over 50 megapixels');
    }
  }
  const out = await encodeImage(file, format);
  return {
    input: {
      name: file.name,
      mime: out.mime,
      kind: 'image',
      data_b64: bytesToBase64(out.bytes),
      thumb: out.thumb,
      w: out.w,
      h: out.h,
    },
    bytes: out.bytes.length,
  };
}

interface Encoded {
  bytes: Uint8Array;
  mime: 'image/png' | 'image/jpeg';
  w: number;
  h: number;
}

/**
 * Decodes the image, with its EXIF orientation applied, and re-encodes it: PNG when that is at most 1 MB, otherwise a JPEG
 * flattened onto the background. A JPEG still over 1.5 MB is tried once more, smaller. The thumbnail is made from the decoded image.
 */
async function encodeImage(file: File, format: ImageFormat): Promise<Encoded & { thumb?: string }> {
  const bitmap = await decode(file, format);
  try {
    if (bitmap.width * bitmap.height > LIMITS.megapixels) {
      throw new Refused('over 50 megapixels');
    }
    let out = await encodeAt(bitmap, 'png', LIMITS.longEdge);
    if (out.bytes.length > LIMITS.pngBytes) {
      out = await encodeAt(bitmap, 'jpeg', LIMITS.longEdge, 0.85);
      if (out.bytes.length > LIMITS.processedImageBytes) {
        out = await encodeAt(bitmap, 'jpeg', LIMITS.retryEdge, 0.7);
      }
      if (out.bytes.length > LIMITS.processedImageBytes) {
        throw new Refused('still larger than 1.5 MB after compression');
      }
    }
    return { ...out, thumb: thumbOf(bitmap) };
  } finally {
    bitmap.close();
  }
}

async function decode(file: File, format: ImageFormat): Promise<ImageBitmap> {
  try {
    return await createImageBitmap(file, { imageOrientation: 'from-image' });
  } catch {
    const formatOnly = format === 'bmp' || format === 'tiff' || format === 'heic';
    throw new Refused(formatOnly ? "this image format can't be read here" : "this image can't be read here");
  }
}

async function encodeAt(bitmap: ImageBitmap, kind: 'png' | 'jpeg', edge: number, quality?: number): Promise<Encoded> {
  const { w, h } = fitWithin(bitmap.width, bitmap.height, edge);
  const canvas = drawCanvas(bitmap, w, h, kind === 'jpeg' ? BACKGROUND : null);
  const mime = kind === 'png' ? 'image/png' : 'image/jpeg';
  const bytes = await canvasBytes(canvas, mime, quality);
  return { bytes, mime, w, h };
}

/** The thumbnail: a JPEG with its long edge at 96 px, at the first quality whose data URL fits in 16 KB. Undefined if none does. */
function thumbOf(bitmap: ImageBitmap): string | undefined {
  const { w, h } = fitWithin(bitmap.width, bitmap.height, LIMITS.thumbEdge);
  const canvas = drawCanvas(bitmap, w, h, BACKGROUND);
  for (const quality of [0.7, 0.5, 0.35]) {
    const url = canvas.toDataURL('image/jpeg', quality);
    if (url.length <= LIMITS.thumbChars) {
      return url;
    }
  }
  return undefined;
}

function drawCanvas(bitmap: ImageBitmap, w: number, h: number, background: string | null): HTMLCanvasElement {
  const canvas = document.createElement('canvas');
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext('2d');
  if (ctx === null) {
    throw new Refused("this image can't be read here");
  }
  if (background !== null) {
    ctx.fillStyle = background;
    ctx.fillRect(0, 0, w, h);
  }
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = 'high';
  ctx.drawImage(bitmap, 0, 0, w, h);
  return canvas;
}

function canvasBytes(canvas: HTMLCanvasElement, mime: string, quality?: number): Promise<Uint8Array> {
  return new Promise((resolve, reject) => {
    canvas.toBlob(
      (blob) => {
        if (blob === null) {
          reject(new Refused("this image can't be read here"));
          return;
        }
        blob.arrayBuffer().then((buffer) => resolve(new Uint8Array(buffer)), reject);
      },
      mime,
      quality,
    );
  });
}

/**
 * The files in a paste. Some engines list a pasted file (a screenshot, a copied file) only among `items`, so `files` is
 * checked first and `items` (kind 'file') is the fallback. Empty when the paste carries no file.
 */
export function filesFromPaste(data: Pick<DataTransfer, 'files' | 'items'> | null | undefined): File[] {
  if (!data) {
    return [];
  }
  const files = Array.from(data.files ?? []);
  if (files.length > 0) {
    return files;
  }
  const out: File[] = [];
  for (const item of Array.from(data.items ?? [])) {
    if (item.kind === 'file') {
      const file = item.getAsFile();
      if (file !== null) {
        out.push(file);
      }
    }
  }
  return out;
}

/** The attachments as the transcript shows them (the sent message, and the restored one). */
export function userAttachments(inputs: AttachmentInput[]): UserAttachment[] | undefined {
  if (inputs.length === 0) {
    return undefined;
  }
  return inputs.map((input) => ({
    kind: input.kind === 'image' ? 'image' : 'file',
    name: input.name,
    bytes: decodedSize(input.data_b64),
    thumb: input.thumb,
  }));
}
