use anyhow::{Result, bail};
use sn_services::Services;
use sn_services::remna_transfer::{Direction, Mode, Plan, Progress, RunEnd, StartOptions};
use std::time::Duration;

fn n(value: u64) -> String {
    sn_domain::format::group_digits(value)
}

fn examples(title: &str, count: u64, list: &[sn_services::remna_transfer::Example]) {
    if count == 0 {
        return;
    }
    println!("{title}: {}", n(count));
    for example in list {
        println!("  · {}", example.who);
        for line in &example.details {
            println!("      {line}");
        }
    }
}

pub fn print_plan(plan: &Plan) {
    println!(
        "Пользователей в Remnawave: {}. Просмотрено: {}.",
        n(plan.remna_users),
        n(plan.scanned)
    );
    match plan.direction {
        Direction::Push => {
            examples(
                "Создать в Remnawave",
                plan.create.count,
                &plan.create.examples,
            );
            if plan.create.count > 0 {
                println!(
                    "  ключи протоколов: прежние у {}, новые у {}; уже истекли: {}",
                    n(plan.keys_kept),
                    n(plan.keys_new),
                    n(plan.expired)
                );
            }
            examples("Данные расходятся", plan.fix.count, &plan.fix.examples);
            for (field, count) in &plan.fields {
                println!("  {field}: {}", n(*count));
            }
            if plan.later_in_remna > 0 {
                println!(
                    "В Remnawave срок позже, чем в панели, — не сокращаем: {}",
                    n(plan.later_in_remna)
                );
            }
            examples(
                "Ссылка сменится",
                plan.link_change.count,
                &plan.link_change.examples,
            );
            for squad in &plan.squads {
                let name = squad.name.clone().unwrap_or_else(|| squad.old.to_string());
                let target = squad
                    .suggested
                    .and_then(|uuid| plan.remna_squads.iter().find(|s| s.uuid == uuid))
                    .map_or_else(
                        || "нет пары — сквад не добавится, выберите замену в админке".to_string(),
                        |s| format!("заменим на «{}» с тем же именем", s.name),
                    );
                println!(
                    "Сквада «{name}» нет в этой Remnawave ({} подписок): {target}",
                    n(squad.subscriptions)
                );
            }
        }
        Direction::Pull => {
            examples(
                "Новые клиенты с подпиской",
                plan.create.count,
                &plan.create.examples,
            );
            examples(
                "Подписка к найденному клиенту",
                plan.attach.count,
                &plan.attach.examples,
            );
            for (by, count) in &plan.matched_by {
                println!("  найдены по {by}: {}", n(*count));
            }
            println!(
                "Тариф подобран: {}, без тарифа: {}",
                n(plan.tariff_matched),
                n(plan.tariff_none)
            );
        }
    }
    examples("Привязать заново", plan.relink.count, &plan.relink.examples);
    println!("Уже совпадают: {}", n(plan.same));
    for reason in &plan.skipped {
        println!("Пропустим {} — {}", n(reason.count), reason.text);
        for example in &reason.examples {
            println!("  · {example}");
        }
    }
    for warning in &plan.warnings {
        println!("Внимание: {warning}");
    }
}

pub fn print_progress(direction: Direction, progress: &Progress) {
    let line = match direction {
        Direction::Push => format!(
            "создано {}, исправлено {}, привязано заново {}, без изменений {}, пропущено {}, ошибок {}",
            n(progress.created),
            n(progress.fixed),
            n(progress.relinked),
            n(progress.same),
            n(progress.skipped),
            n(progress.failed)
        ),
        Direction::Pull => format!(
            "новых подписок {}, новых клиентов {}, привязано заново {}, без изменений {}, пропущено {}, ошибок {}",
            n(progress.attached),
            n(progress.clients),
            n(progress.relinked),
            n(progress.same),
            n(progress.skipped),
            n(progress.failed)
        ),
    };
    println!("{} из {}: {line}", n(progress.done), n(progress.total));
}

pub async fn run(services: &Services, pull: bool, dry_run: bool, fix: bool) -> Result<()> {
    let direction = if pull {
        Direction::Pull
    } else {
        Direction::Push
    };
    if services.remna().is_err() {
        bail!("Remnawave не подключена: задайте адрес и токен в «Настройки → Remnawave»");
    }
    let ops = services.remna_transfer();
    if dry_run {
        let (plan, _) = ops.preview(direction).await?;
        print_plan(&plan);
        println!("Проверка без записи: ни панель, ни Remnawave не изменены.");
        return Ok(());
    }
    let checked = ops.check_now(direction, None).await?;
    let Some(plan) = &checked.plan else {
        bail!(
            "{}",
            checked
                .error
                .unwrap_or_else(|| "проверка не удалась".to_string())
        );
    };
    print_plan(plan);
    let options = StartOptions {
        mode: if fix { Mode::Fix } else { Mode::Create },
        ..StartOptions::default()
    };
    let id = checked.id;
    let watcher = {
        let services = services.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                if let Ok(view) = services.remna_transfer().get(id).await
                    && let Some(progress) = view.progress
                {
                    print_progress(direction, &progress);
                }
            }
        })
    };
    let end = ops.start_now(id, options, None).await;
    watcher.abort();
    match end? {
        RunEnd::Done(progress) => {
            print_progress(direction, &progress);
            if progress.link_changed > 0 {
                println!(
                    "У {} клиентов ссылка сменилась: разошлите им новую в «Настройки → Remnawave → Синхронизация».",
                    n(progress.link_changed)
                );
            }
            println!("Готово. Отчёт — в «Настройки → Remnawave → Синхронизация».");
            Ok(())
        }
        RunEnd::Paused(progress) => {
            print_progress(direction, &progress);
            println!(
                "Остановлено. Продолжите в админке или запустите команду снова — дублей не будет."
            );
            Ok(())
        }
        RunEnd::Stopped(progress, text) => {
            print_progress(direction, &progress);
            bail!("{text}. Сделанное сохранено — запустите команду снова, дублей не будет")
        }
    }
}
