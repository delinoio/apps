// SPDX-License-Identifier: FSL-1.1-ALv2
use super::provider_events::*;
use super::{Fixture, key};

/// Deliver the same payment, past-due and cancellation occurrences in the given order.
async fn cancelled_after_past_due(f: &Fixture, s: &Subscriber, payment_first: bool) {
    if payment_first {
        initial_payment(f, s).await;
    }
    subscription_status(f, s, "past_due", s.after(10)).await;
    subscription_status(f, s, "canceled", s.after(20)).await;
    if !payment_first {
        initial_payment(f, s).await;
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn confirmed_cancellation_clears_renewal_failure_in_any_delivery_order() {
    for payment_first in [true, false] {
        let f = Fixture::setup().await;
        let s = subscriber(&f).await;
        cancelled_after_past_due(&f, &s, payment_first).await;

        let free = default_free_plan(&f).await;
        assert_eq!(tenant_state(&f, &s).await, (free, String::new(), false));
        assert!(!tenant_blocked(&f, &s).await);
        let result = authorize_paid(&f, &s).await;
        assert_eq!(result["allowed"], true, "payment_first={payment_first}");
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn cancellation_keeps_other_restrictions() {
    let f = Fixture::setup().await;
    let s = subscriber(&f).await;
    cancelled_after_past_due(&f, &s, true).await;

    sqlx::query("UPDATE tenants SET operator_block=true WHERE id=$1::text::uuid")
        .bind(&s.tenant)
        .execute(&f.db)
        .await
        .unwrap();
    assert!(tenant_blocked(&f, &s).await);
    assert_ne!(authorize_paid(&f, &s).await["allowed"], true);
    sqlx::query("UPDATE tenants SET operator_block=false WHERE id=$1::text::uuid")
        .bind(&s.tenant)
        .execute(&f.db)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO disputes(external_id,tenant_id,active) VALUES($1,$2::text::uuid,true)",
    )
    .bind(key())
    .bind(&s.tenant)
    .execute(&f.db)
    .await
    .unwrap();
    assert!(tenant_blocked(&f, &s).await);
    assert_ne!(authorize_paid(&f, &s).await["allowed"], true);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn active_status_and_stale_notifications_keep_unresolved_failure() {
    let f = Fixture::setup().await;
    let s = subscriber(&f).await;
    initial_payment(&f, &s).await;
    subscription_status(&f, &s, "past_due", s.after(10)).await;
    subscription_status(&f, &s, "active", s.after(20)).await;
    assert!(tenant_state(&f, &s).await.2);
    assert!(tenant_blocked(&f, &s).await);

    subscription_status(&f, &s, "canceled", s.after(30)).await;
    // A stale past-due and a duplicate cancellation must not restore the failure.
    subscription_status(&f, &s, "past_due", s.after(15)).await;
    subscription_status(&f, &s, "canceled", s.after(30)).await;
    let free = default_free_plan(&f).await;
    assert_eq!(tenant_state(&f, &s).await, (free, String::new(), false));
    assert_eq!(authorize_paid(&f, &s).await["allowed"], true);
}
