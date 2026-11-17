import { useMemo } from 'react';
import { encode } from 'uqr';

export function QrCode({ text, size, label }: { text: string; size: number; label: string }) {
  const code = useMemo(() => encode(text, { border: 2 }), [text]);
  const path = useMemo(() => {
    const parts: string[] = [];
    code.data.forEach((row, y) => {
      row.forEach((dark, x) => {
        if (dark) {
          parts.push(`M${x} ${y}h1v1h-1z`);
        }
      });
    });
    return parts.join('');
  }, [code]);
  return (
    <svg viewBox={`0 0 ${code.size} ${code.size}`} width={size} height={size} role="img" aria-label={label} className="qr">
      <rect width={code.size} height={code.size} fill="#ffffff" />
      <path d={path} fill="#000000" shapeRendering="crispEdges" />
    </svg>
  );
}
