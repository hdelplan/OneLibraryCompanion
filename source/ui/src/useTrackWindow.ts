import { useEffect, useLayoutEffect, useRef, useState } from "react";
// One continuous list; only rows near the viewport need DOM or artwork requests.
export function useTrackWindow(
  count: number,
  result: unknown,
  active: boolean,
  rowHeight = 52,
) {
  const ref = useRef<HTMLDivElement>(null);
  const [top, setTop] = useState(0);
  const savedTop = useRef(0);
  const [height, setHeight] = useState(500);
  useLayoutEffect(() => {
    savedTop.current = 0;
    if (ref.current) ref.current.scrollTop = 0;
    setTop(0);
  }, [result]);
  useLayoutEffect(() => {
    if (active && ref.current) ref.current.scrollTop = savedTop.current;
  }, [active, count, rowHeight]);
  useEffect(() => {
    const element = ref.current;
    if (!element || !active) return;
    const observer = new ResizeObserver(() => setHeight(element.clientHeight));
    observer.observe(element);
    setHeight(element.clientHeight);
    return () => observer.disconnect();
  }, [active]);
  const start = Math.min(
    count,
    Math.max(0, Math.floor((top - 43) / rowHeight) - 6),
  );
  const end = Math.min(count, start + Math.ceil(height / rowHeight) + 14);
  return {
    ref,
    start,
    end,
    onScroll: () => {
      if (!active) return;
      savedTop.current = ref.current?.scrollTop ?? 0;
      setTop(savedTop.current);
    },
  };
}
