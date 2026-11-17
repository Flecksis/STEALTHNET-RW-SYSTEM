type Props = {
  title: string;
  text?: string;
};

export function Pending({ title, text }: Props) {
  return (
    <>
      <h1 className="page-title">{title}</h1>
      <section className="card card-pad" style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        <h2 className="section-title">Раздел в работе</h2>
        <p className="muted">{text ?? 'Экран этого раздела ещё собирается по макету. Данные и действия появятся здесь после обновления.'}</p>
      </section>
    </>
  );
}
