pub fn by_code(code: &str) -> Option<&'static str> {
    Some(match code {
        "not_found" | "remnawave.not_found" => "Not found.",
        "auth.required" => "Please sign in again.",
        "auth.forbidden" => "Access denied.",
        "auth.rate_limited" => "Too many attempts. Please wait a little and try again.",
        "auth.code_attempts" => "Too many wrong codes. Request a new code and try again.",
        "auth.code_invalid" => "The code is wrong or has expired.",
        "auth.email_code_mode" => "Email sign-in works with a code from the email.",
        "auth.email_password_mode" => "Email sign-in works with a password, no code needed.",
        "auth.email_taken" => "This email is already registered. Sign in with your password.",
        "auth.login_expired" => "The sign-in has expired. Please start again.",
        "auth.miniapp_off" => "The mini app is turned off. Please use the bot.",
        "auth.no_email" => {
            "No email is linked to this account, so a website password isn't needed."
        }
        "auth.reset_unavailable" => "The password can't be reset by email. Please contact support.",
        "auth.same_password" => "The new password is the same as the old one.",
        "auth.site_off" => "Website sign-in is turned off. Please use the bot.",
        "auth.totp_code" => "The code didn't work. Check the time on your phone.",
        "auth.totp_disabled" => "Two-step sign-in isn't turned on.",
        "auth.totp_enabled" => "Two-step sign-in is already on.",
        "auth.totp_secret" => "The setup key is damaged. Start the setup again.",
        "auth.totp_required" => "Enter the code from your authenticator app.",
        "auth.totp_invalid" => "The code didn't work. Check the time on your phone.",
        "auth.weak_password" => "The password must be 8 to 128 characters long.",
        "auth.wrong_password" => "Wrong email or password.",
        "auth.oauth_failed" => "Sign-in didn't go through. Please try again.",
        "antibot.expired" => "The button has expired. Request the free trial again.",
        "antibot.too_fast" => "Tap the button once more.",
        "antibot.confirm" => "First confirm you're not a robot.",
        "balance.insufficient" => "Not enough money on your balance.",
        "balance.topup_out_of_range" => "This top-up amount isn't allowed.",
        "balance.currency" => "The balance is kept in a different currency.",
        "checkout.balance_currency" => {
            "Only amounts in the shop currency can be paid from the balance."
        }
        "checkout.method_unavailable" => "This payment method isn't available here.",
        "checkout.topup_internal" => "Top-ups are paid through a payment system only.",
        "client.action_off" => "You can't do this yourself. Please contact support.",
        "client.locale" => "This language is turned off right now.",
        "config.public_url" => "Payments aren't set up yet. Please contact support.",
        "gram.closed" => "This invoice is already closed. Please start the payment again.",
        "module.disabled" => "This feature is turned off right now.",
        "module.unavailable" => "This feature is unavailable right now.",
        "not_implemented" => "This feature isn't available yet. Try later or contact support.",
        "order.awaiting_payment" => "The payment hasn't arrived yet. Please wait a little.",
        "order.delivery_running" => "This order is already being delivered. Please wait.",
        "payment.email_required" => "This payment method needs an email. Choose another one.",
        "payment.error" => {
            "The payment system didn't respond. Try another method or try again later."
        }
        "promo.disabled" => "Promo codes aren't accepted right now.",
        "remnawave.not_configured" | "remnawave.unavailable" | "remnawave.error" => {
            "The VPN service is unavailable right now. Try again in a few minutes."
        }
        "roulette.blocked" => "Your account is blocked.",
        "roulette.empty" => "No prizes left for today. Come back later.",
        "roulette.no_spins" => "No spins yet.",
        "roulette.not_ready" => "The lucky spin isn't set up yet. Come back later.",
        "stars.foreign_invoice" => "This invoice belongs to another account.",
        "stars.invoice_mismatch" => "The invoice has expired. Please start again.",
        "stars.unknown_invoice" => "Invoice not found.",
        "subscription.disabled" => {
            "The subscription is blocked, so payment isn't available. Please contact support."
        }
        "subscription.ended" => "The subscription has ended.",
        "support.body" => "The message is empty or too long.",
        "support.closed" => "This request is closed. Please open a new one.",
        "support.too_many" => "You have too many open requests. Please wait for a reply.",
        "tariff.period_not_found" => "This period isn't available.",
        "tariff.period_inactive" => "This period isn't on sale.",
        "tariff.not_on_sale" => "This plan isn't on sale.",
        "tariff.no_price" => "This period has no price in the shop currency.",
        "tariff.no_stars_price" => "This period can't be paid with stars.",
        "tariff.extra_devices_not_sold" => "Extra devices aren't sold for this plan.",
        "tariff.too_many_devices" => "That's more extra devices than allowed.",
        "topup.currency" => "The balance is topped up in a different currency.",
        "trial.disabled" => "The free trial isn't available right now.",
        "trial.has_subscription" => {
            "The free trial is only available before your first subscription."
        }
        "trial.no_identity" => "The free trial needs an account with Telegram or email.",
        "trial.not_configured" => "The free trial isn't set up yet.",
        "trial.used" => "You've already used the free trial.",
        "addon.no_tariff" => "This subscription has no plan.",
        "addon.unlimited" => "This subscription has unlimited traffic, nothing to add.",
        "addon.wait" => "The previous order is still being delivered. Please wait a minute.",
        "mail.address" => "The email address looks wrong.",
        "mail.unavailable" | "mail.failed" => "We couldn't send the email. Please try again later.",
        "referral.withdrawals_off" => "Withdrawals aren't accepted right now.",
        "referral.method_unavailable" => {
            "This withdrawal method isn't available right now. Choose another one."
        }
        "referral.bad_amount" => "The amount looks wrong.",
        "referral.below_minimum" => "The amount is below the minimum.",
        "referral.not_enough" => "There isn't enough on your referral balance.",
        "referral.bad_details" => "The payout details look wrong.",
        "referral.already_decided" => "This request has already been processed.",
        "validation.invalid_value" | "validation.unknown_variant" => "Some data looks wrong.",
        "internal" => "Something went wrong. Please try again.",
        _ => return None,
    })
}

pub fn message(code: &str) -> Option<String> {
    by_code(code).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn английские_ошибки_без_кириллицы_и_с_точкой() {
        for code in [
            "auth.rate_limited",
            "trial.used",
            "payment.error",
            "support.closed",
            "remnawave.error",
        ] {
            let text = by_code(code).unwrap();
            assert!(text.ends_with('.'), "{code}");
            assert!(!text.chars().any(|c| ('а'..='я').contains(&c)), "{code}");
        }
        assert_eq!(by_code("admins.exists"), None);
    }
}
