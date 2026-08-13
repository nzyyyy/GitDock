import { useCallback, useRef, useState } from "react";
import { api } from "../api";
import { appendLog, lastLog, newLogBuffer, readLogs, type LogBuffer, type LogLine } from "../lib/logBuffer";
import { errorMessage } from "../types";

export function useLogBuffer({ setOutputOpen }: { setOutputOpen: React.Dispatch<React.SetStateAction<boolean>> }) {
  const logBuffer = useRef<LogBuffer>(newLogBuffer());
  const [, setLogRevision] = useState(0);
  const pushLog = useCallback((kind: LogLine["kind"], message: string) => {
    if (appendLog(logBuffer.current, { id: Date.now() + Math.random(), timestamp: new Date().toISOString(), kind, message })) {
      setLogRevision((current) => current + 1);
    }
  }, []);

  const exportLogs = useCallback(async () => {
    try {
      const stamp = new Date().toISOString().replaceAll(":", "-").replace(/\.\d{3}Z$/, "Z");
      await api.exportSessionLog(`gitdock-session-${stamp}.log`, readLogs(logBuffer.current).map(({ timestamp, kind, message }) => ({ timestamp, kind, message })));
    } catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
  }, [pushLog, setOutputOpen]);

  const clearLogs = () => { logBuffer.current = newLogBuffer(); setLogRevision((current) => current + 1); };
  const logCount = logBuffer.current.length;

  return { pushLog, exportLogs, clearLogs, logCount, lastLog: lastLog(logBuffer.current), logBuffer };
}
