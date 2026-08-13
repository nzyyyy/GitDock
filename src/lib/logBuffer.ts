import type { SessionLogLine } from "../api";

export type LogLine = SessionLogLine & { id: number; bytes: number };
export type LogBuffer = { entries: Array<LogLine | undefined>; start: number; length: number; bytes: number };

const LOG_LIMIT = 10_000;
const LOG_BYTE_LIMIT = 5 * 1024 * 1024;
const textEncoder = new TextEncoder();

export const newLogBuffer = (): LogBuffer => ({ entries: [], start: 0, length: 0, bytes: 0 });

export const appendLog = (buffer: LogBuffer, line: Omit<LogLine, "bytes">) => {
  const bytes = textEncoder.encode(`${line.timestamp} ${line.kind} ${line.message}\n`).byteLength;
  if (bytes > LOG_BYTE_LIMIT) return false;
  while (buffer.length && (buffer.length >= LOG_LIMIT || buffer.bytes + bytes > LOG_BYTE_LIMIT)) {
    buffer.bytes -= buffer.entries[buffer.start]!.bytes;
    buffer.entries[buffer.start] = undefined;
    buffer.start = (buffer.start + 1) % LOG_LIMIT;
    buffer.length -= 1;
  }
  const index = (buffer.start + buffer.length) % LOG_LIMIT;
  buffer.entries[index] = { ...line, bytes };
  buffer.length += 1;
  buffer.bytes += bytes;
  return true;
};

export const readLogs = (buffer: LogBuffer) => Array.from({ length: buffer.length }, (_, index) => buffer.entries[(buffer.start + index) % LOG_LIMIT]!);
export const lastLog = (buffer: LogBuffer) => buffer.length ? buffer.entries[(buffer.start + buffer.length - 1) % LOG_LIMIT] : undefined;
