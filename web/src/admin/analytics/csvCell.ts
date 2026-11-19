const FORMULA = /^[=+\-@\t\r]/;
const NUMBER = /^-?\d+(?:[.,]\d+)?$/;

export function csvCell(value: string | number | null): string {
  if (value === null) {
    return '';
  }
  const raw = String(value);
  const text = FORMULA.test(raw) && !NUMBER.test(raw) ? `'${raw}` : raw;
  return /[;"\n\r]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
}
