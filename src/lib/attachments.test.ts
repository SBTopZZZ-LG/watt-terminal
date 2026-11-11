import { describe, expect, it } from 'vitest';
import {
  addLoad,
  admitRefusal,
  bytesToBase64,
  countRefusal,
  decodedSize,
  filesFromPaste,
  fitWithin,
  formatSize,
  imageFormatOf,
  isPdf,
  isTextBytes,
  LIMITS,
  looksTruncated,
  loadOf,
  NO_LOAD,
  sniffDims,
  userAttachments,
} from './attachments';

const ascii = (text: string) => Array.from(text, (c) => c.charCodeAt(0));
const bytes = (...parts: Array<number[] | string | Uint8Array>) =>
  new Uint8Array(parts.flatMap((part) => (typeof part === 'string' ? ascii(part) : Array.from(part))));
/** Padding that keeps a short WebP fixture past the 30 bytes the header reader needs. */
const PAD = new Array(8).fill(0);
const le16 = (n: number) => [n & 0xff, (n >> 8) & 0xff];
const le24 = (n: number) => [n & 0xff, (n >> 8) & 0xff, (n >> 16) & 0xff];
const le32 = (n: number) => [n & 0xff, (n >>> 8) & 0xff, (n >>> 16) & 0xff, (n >>> 24) & 0xff];
const be32 = (n: number) => [(n >>> 24) & 0xff, (n >>> 16) & 0xff, (n >>> 8) & 0xff, n & 0xff];
const PNG_SIG = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const IEND = [0, 0, 0, 0, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82];

/** A PNG header of the given size, then an IEND chunk. */
function png(w: number, h: number, ending: number[] = IEND): Uint8Array {
  return bytes(PNG_SIG, [0, 0, 0, 13], 'IHDR', be32(w), be32(h), [8, 6, 0, 0, 0, 0, 0, 0, 0], ending);
}

/** A JPEG with a JFIF segment, an optional DHT before the frame, a start-of-frame of the given size, then EOI. */
function jpeg(w: number, h: number, { dht = false, ending = [0xff, 0xd9] }: { dht?: boolean; ending?: number[] } = {}): Uint8Array {
  const app0 = [0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00];
  const dhtSeg = dht ? [0xff, 0xc4, 0x00, 0x03, 0x00] : [];
  const sof = [0xff, 0xc0, 0x00, 0x11, 0x08, (h >> 8) & 0xff, h & 0xff, (w >> 8) & 0xff, w & 0xff, 0x03, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1];
  return bytes([0xff, 0xd8], app0, dhtSeg, sof, ending);
}

/** A WebP lossless (VP8L) frame: the spec's packing of width-1 and height-1 into 14-bit fields, little-endian. */
function vp8l(w: number, h: number): number[] {
  const value = (w - 1) | ((h - 1) << 14);
  return [0x2f, ...le32(value)];
}

describe('imageFormatOf', () => {
  it('recognises the image signatures', () => {
    expect(imageFormatOf(png(1, 1))).toBe('png');
    expect(imageFormatOf(jpeg(1, 1))).toBe('jpeg');
    expect(imageFormatOf(bytes('GIF89a', le16(1), le16(1)))).toBe('gif');
    expect(imageFormatOf(bytes('GIF87a', le16(1), le16(1)))).toBe('gif');
    expect(imageFormatOf(bytes('RIFF', le32(20), 'WEBPVP8 '))).toBe('webp');
    expect(imageFormatOf(bytes([0x49, 0x49, 0x2a, 0x00, 0, 0, 0, 0]))).toBe('tiff');
    expect(imageFormatOf(bytes([0x4d, 0x4d, 0x00, 0x2a, 0, 0, 0, 0]))).toBe('tiff');
    expect(imageFormatOf(bytes([0, 0, 0, 0x18], 'ftypheic', [0, 0, 0, 0]))).toBe('heic');
    expect(imageFormatOf(bytes([0, 0, 0, 0x18], 'ftypmif1', [0, 0, 0, 0]))).toBe('heic');
  });

  it('recognises a BMP header (zero reserved words, a pixel offset past the file header)', () => {
    expect(imageFormatOf(bytes('BM', le32(1000), [0, 0, 0, 0], le32(54)))).toBe('bmp');
  });

  it('does not take a text file that starts with the same letters', () => {
    expect(imageFormatOf(bytes('BMW notes, 2026'))).toBeNull();
    expect(imageFormatOf(bytes('GIF is a format'))).toBeNull();
    expect(imageFormatOf(bytes('RIFF of the day WEBPish'))).toBeNull();
  });

  it('avif and other ftyp brands are not an image this app takes', () => {
    expect(imageFormatOf(bytes([0, 0, 0, 0x18], 'ftypavif', [0, 0, 0, 0]))).toBeNull();
  });

  it('a PDF is recognised so that it can be refused with its own message', () => {
    expect(isPdf(bytes('%PDF-1.7\n'))).toBe(true);
    expect(isPdf(bytes('hello'))).toBe(false);
  });
});

describe('sniffDims', () => {
  it('reads a PNG header', () => {
    expect(sniffDims(png(640, 480), 'png')).toEqual({ w: 640, h: 480 });
  });

  it('reads a GIF header (little-endian)', () => {
    expect(sniffDims(bytes('GIF89a', le16(640), le16(480)), 'gif')).toEqual({ w: 640, h: 480 });
  });

  it('reads a JPEG start-of-frame, skipping the segments before it', () => {
    expect(sniffDims(jpeg(640, 480), 'jpeg')).toEqual({ w: 640, h: 480 });
    expect(sniffDims(jpeg(1234, 77, { dht: true }), 'jpeg')).toEqual({ w: 1234, h: 77 });
  });

  it('gives up on a JPEG with no frame header', () => {
    expect(sniffDims(bytes([0xff, 0xd8, 0xff, 0xd9]), 'jpeg')).toBeNull();
  });

  it('reads the VP8X (extended) layout: canvas size minus one, as 24-bit numbers', () => {
    const b = bytes('RIFF', le32(22), 'WEBP', 'VP8X', le32(10), [0, 0, 0, 0], le24(639), le24(479));
    expect(sniffDims(b, 'webp')).toEqual({ w: 640, h: 480 });
    const big = bytes('RIFF', le32(22), 'WEBP', 'VP8X', le32(10), [0, 0, 0, 0], le24(3999), le24(2999));
    expect(sniffDims(big, 'webp')).toEqual({ w: 4000, h: 3000 });
  });

  it('reads the VP8 (lossy) layout: 14-bit size after the key-frame start code, scale bits masked off', () => {
    // width 640 with a scale of 1 in the top bits (0x4280), height 480 with a scale of 2 (0x81e0)
    const b = bytes('RIFF', le32(22), 'WEBP', 'VP8 ', le32(10), [0, 0, 0], [0x9d, 0x01, 0x2a], [0x80, 0x42], [0xe0, 0x81]);
    expect(sniffDims(b, 'webp')).toEqual({ w: 640, h: 480 });
  });

  it('gives up on a VP8 frame with no start code', () => {
    const b = bytes('RIFF', le32(22), 'WEBP', 'VP8 ', le32(10), [0, 0, 0], [0, 0, 0], [0x80, 0x02], [0xe0, 0x01]);
    expect(sniffDims(b, 'webp')).toBeNull();
  });

  it('reads the VP8L (lossless) layout: 14-bit width-1 and height-1, packed', () => {
    expect(sniffDims(bytes('RIFF', le32(22), 'WEBP', 'VP8L', le32(10), vp8l(640, 480), PAD), 'webp')).toEqual({ w: 640, h: 480 });
    expect(sniffDims(bytes('RIFF', le32(22), 'WEBP', 'VP8L', le32(10), vp8l(1000, 3000), PAD), 'webp')).toEqual({ w: 1000, h: 3000 });
    expect(sniffDims(bytes('RIFF', le32(22), 'WEBP', 'VP8L', le32(10), vp8l(16384, 2), PAD), 'webp')).toEqual({ w: 16384, h: 2 });
  });

  it('is null for a format it does not read', () => {
    expect(sniffDims(png(1, 1), 'bmp')).toBeNull();
  });
});

describe('looksTruncated', () => {
  it('a JPEG that ends with EOI is complete, with trailing zero padding allowed', () => {
    expect(looksTruncated(jpeg(8, 8), 'jpeg')).toBe(false);
    expect(looksTruncated(bytes(jpeg(8, 8), [0, 0, 0]), 'jpeg')).toBe(false);
  });

  it('a JPEG that stops before EOI is truncated', () => {
    const full = jpeg(8, 8);
    expect(looksTruncated(full.subarray(0, full.length - 20), 'jpeg')).toBe(true);
    expect(looksTruncated(full.subarray(0, full.length - 1), 'jpeg')).toBe(true);
  });

  it('a PNG that ends with its IEND chunk is complete, a few trailing bytes tolerated', () => {
    expect(looksTruncated(png(8, 8), 'png')).toBe(false);
    expect(looksTruncated(bytes(png(8, 8), [1, 2, 3]), 'png')).toBe(false);
  });

  it('a PNG cut before IEND, or inside its CRC, is truncated', () => {
    const full = png(8, 8);
    expect(looksTruncated(full.subarray(0, full.length - 12), 'png')).toBe(true);
    expect(looksTruncated(full.subarray(0, full.length - 3), 'png')).toBe(true);
    expect(looksTruncated(full.subarray(0, full.length - 8), 'png')).toBe(true);
  });

  it('other formats are not checked here', () => {
    expect(looksTruncated(bytes('GIF89a'), 'gif')).toBe(false);
  });
});

describe('isTextBytes', () => {
  it('UTF-8 text is text, multi-byte characters included', () => {
    expect(isTextBytes(new TextEncoder().encode('hello, wörld ✓ 日本'))).toBe(true);
    expect(isTextBytes(new Uint8Array(0))).toBe(true);
  });

  it('a NUL byte makes it binary', () => {
    expect(isTextBytes(bytes('ab', [0], 'cd'))).toBe(false);
  });

  it('bytes that are not UTF-8 are refused (strict decode)', () => {
    expect(isTextBytes(bytes([0xff, 0xfe, 0x41]))).toBe(false);
    expect(isTextBytes(bytes([0xc3]))).toBe(false);
  });
});

describe('fitWithin', () => {
  it('scales the long edge down to the edge, keeping the aspect ratio', () => {
    expect(fitWithin(6000, 4000, LIMITS.longEdge)).toEqual({ w: 1568, h: 1045 });
    expect(fitWithin(4000, 6000, LIMITS.longEdge)).toEqual({ w: 1045, h: 1568 });
  });

  it('never upscales', () => {
    expect(fitWithin(800, 600, LIMITS.longEdge)).toEqual({ w: 800, h: 600 });
  });

  it('keeps at least one pixel on each side', () => {
    expect(fitWithin(1, 100000, 96)).toEqual({ w: 1, h: 96 });
  });
});

describe('base64 and sizes', () => {
  it('decodedSize matches the bytes that were encoded, with padding', () => {
    for (const n of [0, 1, 2, 3, 4, 5, 100, 1001]) {
      const data = new Uint8Array(n).map((_, i) => (i * 7) & 0xff);
      expect(decodedSize(bytesToBase64(data))).toBe(n);
    }
    expect(bytesToBase64(bytes('hello'))).toBe('aGVsbG8=');
  });

  it('bytesToBase64 round-trips a large array (past one chunk)', () => {
    const data = new Uint8Array(200_000).map((_, i) => (i * 31) & 0xff);
    const back = Uint8Array.from(atob(bytesToBase64(data)), (c) => c.charCodeAt(0));
    expect(back).toEqual(data);
  });

  it('formatSize: bytes, KB and MB', () => {
    expect(formatSize(0)).toBe('0 B');
    expect(formatSize(512)).toBe('512 B');
    expect(formatSize(12 * 1024)).toBe('12 KB');
    expect(formatSize(1.5 * 1024 * 1024)).toBe('1.5 MB');
  });
});

describe('per-message accounting', () => {
  const image = (bytesN: number) => ({ kind: 'image' as const, bytes: bytesN });
  const doc = (bytesN: number) => ({ kind: 'text' as const, bytes: bytesN });

  it('an empty message admits anything within the caps', () => {
    expect(admitRefusal(NO_LOAD, image(100))).toBeNull();
    expect(admitRefusal(NO_LOAD, doc(100))).toBeNull();
  });

  it('refuses a ninth attachment', () => {
    const eight = loadOf(Array.from({ length: LIMITS.attachments }, () => doc(1)));
    expect(countRefusal(LIMITS.attachments - 1)).toBeNull();
    expect(countRefusal(LIMITS.attachments)).toBe('At most 8 attachments per message.');
    expect(admitRefusal(eight, doc(1))).toBe('At most 8 attachments per message.');
  });

  it('refuses a seventh image, but admits a text document', () => {
    const six = loadOf(Array.from({ length: LIMITS.images }, () => image(1)));
    expect(admitRefusal(six, image(1))).toBe('At most 6 images per message.');
    expect(admitRefusal(six, doc(1))).toBeNull();
  });

  it('refuses an image that takes the images past 5 MB, and admits one that just fits', () => {
    const load = loadOf([image(LIMITS.imageBytes - 100)]);
    expect(admitRefusal(load, image(101))).toBe('The images of a message total more than 5 MB.');
    expect(admitRefusal(load, image(100))).toBeNull();
  });

  it('refuses a text document that takes the documents past 200 KB', () => {
    const load = loadOf([doc(LIMITS.docsBytes - 10)]);
    expect(admitRefusal(load, doc(11))).toBe('The text documents of a message total more than 200 KB.');
    expect(admitRefusal(load, doc(10))).toBeNull();
  });

  it('addLoad counts an image in both the count and the image bytes', () => {
    expect(addLoad(NO_LOAD, image(7))).toEqual({ attachments: 1, images: 1, imageBytes: 7, docBytes: 0 });
    expect(addLoad(NO_LOAD, doc(7))).toEqual({ attachments: 1, images: 0, imageBytes: 0, docBytes: 7 });
  });
});

describe('filesFromPaste', () => {
  const file = (name: string) => new File([new Uint8Array([1, 2, 3])], name, { type: '' });
  /** A DataTransferItem-like entry: `kind` and getAsFile, as the engines expose them. */
  const item = (kind: string, f: File | null) => ({ kind, getAsFile: () => f });

  it('no clipboard, or no file in it: empty', () => {
    expect(filesFromPaste(null)).toEqual([]);
    expect(filesFromPaste({ files: [] as unknown as FileList, items: [item('string', null)] as unknown as DataTransferItemList })).toEqual([]);
  });

  it('takes the files list when it has files', () => {
    const a = file('a.png');
    expect(filesFromPaste({ files: [a] as unknown as FileList, items: [] as unknown as DataTransferItemList })).toEqual([a]);
  });

  it('falls back to the items of kind file when the files list is empty (a screenshot paste)', () => {
    const shot = file('shot.png');
    const out = filesFromPaste({
      files: [] as unknown as FileList,
      items: [item('string', null), item('file', shot), item('file', null)] as unknown as DataTransferItemList,
    });
    expect(out).toEqual([shot]);
  });
});

describe('userAttachments', () => {
  it('no attachments: undefined, so the transcript item has none', () => {
    expect(userAttachments([])).toBeUndefined();
  });

  it('maps an image to kind image with its thumbnail, and a text document to kind file', () => {
    const data = bytesToBase64(new Uint8Array(10));
    const out = userAttachments([
      { name: 'shot.png', mime: 'image/png', kind: 'image', data_b64: data, thumb: 'data:image/jpeg;base64,AAAA', w: 4, h: 4 },
      { name: 'notes.md', mime: 'text/plain', kind: 'text', data_b64: bytesToBase64(new Uint8Array(3)) },
    ]);
    expect(out).toEqual([
      { kind: 'image', name: 'shot.png', bytes: 10, thumb: 'data:image/jpeg;base64,AAAA' },
      { kind: 'file', name: 'notes.md', bytes: 3, thumb: undefined },
    ]);
  });
});
