# Close to Kindle navigation

Version 0.1.11 uses an X in the upper-right corner of the Account header to close
BiliComics and return to Kindle. It supersedes the 0.1.10 left-side return arrow.
The Account home and shared Scribe header retain the original left-aligned title,
font size, weight, and separator. The previous bottom exit row and large Scribe
exit panel remain removed.

The close control is available on all Account sections, including Paperwhite
child settings and Scribe's flat tabs. Paperwhite child settings keep the
left-side back-to-Account control for local navigation. The X reuses the existing
IconButton and close asset: a 24px Paperwhite or 28px Scribe icon centered in a
48 by 48px target. It has no visible text or idle border. Its localized accessible
name describes closing BiliComics and returning to Kindle. Press feedback,
visible focus, Enter/Space, and the accessibility action remain provided by the
shared component. The background close control is disabled while the recharge
sheet is visible.

The existing `exit` action invokes the safe quit controller, which saves local
progress and releases native control. Closing is separate from signing out;
there is no new confirmation step or native display-mode change.

All execution checks for the frozen 0.1.11 candidate used `test-env`. The source
passed Cargo check, 458 library tests, and the desktop build. Five XTest cases
verified actual toolbar taps on Paperwhite home, Paperwhite local back followed
by close, and Scribe home, System portrait, and System landscape. Each original
application process exited with code 0 within 300ms of the close tap, and its
owned process group was gone before harness cleanup. Six targeted Account PNGs
were directly inspected for title alignment, the upper-right X, and absence of
the old exit controls. The broader 0.1.10 matrices retain their historical scope.

The ARM development build completed successfully, and its archive passed all
nine static checks for 28 entries. The actual build log, including its warnings,
is retained separately from the package runner's wrapper log. Desktop input and
static-package checks do not establish a new physical-device result. Earlier
physical wake and restoration evidence retains its own candidate scope. The R8
pre-device checks did not acquire a Kindle lease. The later operator-authorized
physical follow-up is recorded separately from those checks.

See the [evidence summary](../artifacts/verification/account-close-20261107/summary.json)
and [visual review](../artifacts/verification/account-close-20261107/visual-review.json).
The [0.1.10 evidence](../artifacts/verification/exit-navigation-20261107/summary.json)
is preserved as the superseded layout's historical record.
