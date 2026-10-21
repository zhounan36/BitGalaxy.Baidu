const TYPES = {
  image: ["IMG", "#22a06b"],
  video: ["VID", "#7b5ce0"],
  audio: ["MP3", "#e8912d"],
  doc: ["DOC", "#2b6de0"],
  zip: ["ZIP", "#d19b26"],
  code: ["DEV", "#2fa7bd"],
  torrent: ["BT", "#8a94a6"],
  other: ["FILE", "#8a94a6"],
};

const EXT_MAP = {
  jpg: "image", jpeg: "image", png: "image", gif: "image", bmp: "image", webp: "image", svg: "image", heic: "image",
  mp4: "video", mkv: "video", avi: "video", mov: "video", wmv: "video", flv: "video", webm: "video", m4v: "video",
  mp3: "audio", flac: "audio", wav: "audio", aac: "audio", ogg: "audio", m4a: "audio", wma: "audio",
  doc: "doc", docx: "doc", pdf: "doc", txt: "doc", md: "doc", xls: "doc", xlsx: "doc", ppt: "doc", pptx: "doc", csv: "doc",
  zip: "zip", rar: "zip", "7z": "zip", tar: "zip", gz: "zip", bz2: "zip", iso: "zip",
  js: "code", ts: "code", tsx: "code", jsx: "code", py: "code", rs: "code", go: "code", java: "code", c: "code", cpp: "code", h: "code", json: "code", yml: "code", yaml: "code", xml: "code", html: "code", css: "code", sh: "code",
  torrent: "torrent",
};

export function fileKind(item) {
  if (item.is_dir) return "dir";
  const ext = (item.name.split(".").pop() || "").toLowerCase();
  return EXT_MAP[ext] || "other";
}

export default function FileIcon({ item, size = 34 }) {
  if (item.is_dir) {
    return (
      <svg width={size} height={size} viewBox="0 0 34 34">
        <path
          d="M3 10a3 3 0 0 1 3-3h7l3 3.5h12a3 3 0 0 1 3 3V26a3 3 0 0 1-3 3H6a3 3 0 0 1-3-3z"
          fill="#5b8def"
        />
        <path d="M3 13h28v3H3z" fill="#ffffff" opacity="0.18" />
      </svg>
    );
  }
  const kind = fileKind(item);
  const [label, color] = TYPES[kind] || TYPES.other;
  return (
    <svg width={size} height={size} viewBox="0 0 34 34">
      <path
        d="M8 3h12l7 7v21a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"
        fill={color}
        opacity="0.92"
      />
      <path d="M20 3l7 7h-7z" fill="#000" opacity="0.18" />
      <text
        x="17"
        y="24"
        textAnchor="middle"
        fontSize="9"
        fontWeight="700"
        fill="#fff"
        fontFamily="system-ui, sans-serif"
      >
        {label}
      </text>
    </svg>
  );
}
