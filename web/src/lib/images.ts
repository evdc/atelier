import type { Image } from "../gen/Image";

const OK = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
/** The model downsamples past this anyway; sending more only costs bytes. */
const MAX_EDGE = 1568;
const MAX_BYTES = 3 * 1024 * 1024;

export const MAX_IMAGES = 8;
export const isImage = (f: File) => OK.has(f.type);
export const dataUrl = (i: Image) => `data:${i.media_type};base64,${i.data}`;

function b64(bytes: ArrayBuffer): string {
  const u = new Uint8Array(bytes);
  let s = "";
  for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode(...u.subarray(i, i + 0x8000));
  return btoa(s);
}

/** A file as a message attachment, shrunk first when it's bigger than the model can use. */
export async function toImage(f: File): Promise<Image> {
  const bmp = await createImageBitmap(f);
  const scale = Math.min(1, MAX_EDGE / Math.max(bmp.width, bmp.height));
  // Animated gifs would lose their frames in a re-encode; send them as they are unless too big.
  if (scale === 1 && f.size <= MAX_BYTES) {
    bmp.close();
    return { media_type: f.type, data: b64(await f.arrayBuffer()) };
  }
  const c = document.createElement("canvas");
  c.width = Math.round(bmp.width * scale);
  c.height = Math.round(bmp.height * scale);
  c.getContext("2d")!.drawImage(bmp, 0, 0, c.width, c.height);
  bmp.close();
  const jpeg = f.type === "image/jpeg" || f.size > MAX_BYTES;
  const blob = await new Promise<Blob | null>((r) => c.toBlob(r, jpeg ? "image/jpeg" : "image/png", 0.88));
  if (!blob) throw new Error("couldn't read that image");
  return { media_type: blob.type, data: b64(await blob.arrayBuffer()) };
}
