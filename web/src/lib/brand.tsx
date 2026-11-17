type Props = {
  name: string;
  logo?: string | null;
  className: string;
};

export function brandInitial(name: string): string {
  const match = name.match(/[\p{L}\p{N}]/u);
  return match ? match[0].toUpperCase() : 'S';
}

export function BrandMark({ name, logo, className }: Props) {
  return (
    <span className={logo ? `${className} brand-logo` : className} aria-hidden="true">
      {logo ? <img src={logo} alt="" draggable={false} /> : brandInitial(name)}
    </span>
  );
}
