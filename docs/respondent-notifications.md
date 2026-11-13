**English** · [Français](fr/respondent-notifications.md)

# Notifying respondents of new and waiting conversations

A new conversation (web chat or SMS) is a room that the bridge creates and to which it **invites**
the active respondents. An invitation by itself often produces **no notification**: it depends on the
push registration of each phone, on the client's settings, and a respondent who has not joined does not
get notified of the messages that follow. This guide explains what the bridge does about it, the one
server setting to turn on, and how to check each respondent's device.

## 1. What the bridge does

1. **New conversation ping.** After creating a room, the bridge waits (up to 20 s) for the invited
   respondents to join, then posts a short bilingual message that **mentions** every respondent who
   has joined: `🔔 Nouveau clavardage Web en attente. / New web chat waiting.` (or "SMS conversation").
   A mention is a *highlight*: it notifies with sound even when the room is set to "mentions only".
   The message also **lists the respondents by name** on a second line (`👥 Alice, Bob, …`, display name
   or `@username`; clickable pills in Element, at most 15 names then `+N`), which makes the
   notification easier to spot on a lock screen.
2. **Reminders.** A conversation is **waiting** from the moment a visitor or caller message arrives
   until a respondent **types or writes** in the room. While it waits, the bridge posts
   `⏰ Personne n’a répondu depuis 1 minute. / Nobody has answered for 1 minute.` after **1 minute,
   2 minutes and 5 minutes**, then **every hour** (1 h, 2 h, 3 h …, at most 24 hourly reminders by
   default). Each reminder mentions the respondents who have joined. **No ping or reminder is posted while a
   respondent is typing in the room** (it stays due and is posted later if nobody answers).
3. **It stops** as soon as any respondent types or sends a message in the room (a command such as
   `!ok` counts), when the chat is ended or deleted, or when the retention period removes the room.
   *Just opening the room does not count*: a respondent must type or write something.
4. **Follow-ups in a conversation already answered** do not restart the fast reminders: once a respondent has
   typed or written in the room, a new message from the caller or visitor gets one gentle reminder
   (`⏰ Le dernier message attend une réponse depuis 10 minutes. / The last message has been waiting for an answer
   for 10 minutes.`) only after `ATTENTION_FOLLOWUP_MIN` minutes (default 10) without an answer, then hourly.
   Before this, a live exchange pinged everyone whenever a reply took more than a minute.
5. Nothing is posted while **nobody but the bot** is in the room (there is no one to notify); the
   first ping is still posted so it is there when a respondent accepts the invitation.

The messages contain **no personal information**: no visitor text, number or name.

## 2. Server setting: make respondents join automatically

Notifications for messages only exist for a room the respondent has **joined**. With Synapse
(version 1.109 or newer), let local users auto-accept invitations. In `homeserver.yaml`:

```yaml
auto_accept_invites:
    enabled: true
    only_for_direct_messages: false
    only_from_local_users: true
```

Restart Synapse. Effects to be aware of: any **local** user can add another local user to a room
without being asked; respondents' room lists fill with every new conversation (they leave or delete
them as before). If you use another homeserver, use its equivalent, or have respondents accept
invitations promptly (the ping and reminders are then only visible after they accept).

## 3. Bridge settings (`.env`)

| Variable | Default | Meaning |
|---|---|---|
| `ATTENTION_ENABLED` | `true` | `false` switches the ping and the reminders off |
| `ATTENTION_JOIN_WAIT_SECS` | `20` | how long to wait for respondents to join a new room before the first ping |
| `ATTENTION_HOURLY_MAX` | `24` | number of hourly reminders after the 1, 2 and 5 minute ones (`0` = none) |

Restart the bridge after changing them.

## 4. Check each respondent's device

The bridge and the server can only notify a device that **registered for push**. Ask each respondent
to check:

**Element X on Android**
- Install it from the **Google Play** store (it then registers with Google's push service), **or**,
  on a phone without Google services (Huawei, de-Googled Android) or a build from F-Droid / GitHub,
  install a **UnifiedPush** distributor such as **ntfy** and choose it in Element X:
  Settings → Notifications → *Push notification provider*.
- Settings → Notifications: *Enable notifications on this device* on; the **Group chats** default
  should be *All messages* (or at least *Mentions and keywords*; the bridge's messages are mentions);
  *Room invitations* on.
- Android settings → Apps → Element X: notifications allowed; battery **Unrestricted**;
  Do Not Disturb not blocking it.

**Element X on iOS:** Settings → Notifications → on for this device and for group chats; iOS Focus
modes must allow Element X.

**Element Desktop (Linux, Windows, macOS):** Settings → Notifications → *Enable desktop
notifications* and *Enable audible notifications*; the operating system must allow notifications for
Element (on Linux a notification daemon must be running); keep Element running (tray) because
the desktop app notifies only while it runs. A desktop client is a good complement, not a
replacement for a phone.

**Check from the server** (administrator). A phone appears in the `pushers` table only when it
registered for push:

```sql
-- Synapse with SQLite or PostgreSQL
select user_name, app_id, device_display_name from pushers order by user_name;
```

Typical application ids: Element (older Android app) `im.vector.app.android`, Element X on iOS
`io.element.elementx.ios.prod`; Element X on Android registers its own id (it starts with
`io.element.android`). A respondent whose Element X device has **no row** is not registered:
fix that device first (above). Registrations are made when the app starts, so reopen the app after
changing the push provider.

## 5. Test

1. Start a web chat (or text the SMS number) with respondents' phones locked.
2. Within a few seconds of them joining, the ping arrives (sound).
3. Do nothing: reminders follow after 1, 2 and 5 minutes. Type a character in the room: they stop.

## 6. Troubleshooting

| Symptom | Cause / fix |
|---|---|
| No notification at all on a phone | the device is not registered for push (section 4): provider, permissions, battery settings; check the `pushers` table |
| Ping posted but no sound / nothing on one phone | that app's notification channel or Focus/Do Not Disturb; Element X setting for group chats; the phone is not registered |
| The ping appears only after the respondent accepts the invitation | `auto_accept_invites` is off or not applied (section 2), or the respondent is not a *local* user |
| Reminders keep coming although someone is handling it | nobody has typed or written in the room; opening it is not enough. Type, or send a message such as `!ok` |
| Too many reminders / unanswered SMS rooms | lower `ATTENTION_HOURLY_MAX` (or `0`), or `ATTENTION_ENABLED=false` |
| Desktop shows no notification | the app is closed, desktop notifications are off, or no notification daemon (Linux) |
