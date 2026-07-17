"use strict";
(function () {
  const jsonWebAuthnSupport =
    !!globalThis.PublicKeyCredential?.parseCreationOptionsFromJSON;

  /// Render a user-visible message into the login form's error slot (the
  /// `#error_message` div, present only on /login). Null-guarded so a stray
  /// call on any other page is a silent no-op, never a throw.
  function show_error(message) {
    const slot = document.getElementById("error_message");
    if (slot) {
      slot.textContent = message;
    }
  }

  /// Map a passkey-ceremony rejection to human copy. The whole point of Phase
  /// DM: a `navigator.credentials.{create,get}()` rejection used to vanish
  /// (unhandled promise rejection → no DOM write, no console on a phone). Now
  /// every rejection lands here.
  ///
  /// `NotAllowedError`/`AbortError` is the COMMON benign case — the user
  /// dismissed or cancelled the sheet, or it timed out — and reads as "try
  /// again", not "broken". A thrown `Error` carrying server text (a non-OK
  /// start/finish response) shows that text verbatim, so "that name's taken"
  /// reaches the user. Everything else degrades to a generic-but-honest line.
  function ceremony_error_message(err, action) {
    const name = err && err.name;
    if (name === "NotAllowedError" || name === "AbortError") {
      return action + " was cancelled or timed out. Please try again.";
    }
    if (name === "InvalidStateError") {
      return (
        "A passkey for this site already exists on this device — " +
        "try logging in instead of registering."
      );
    }
    if (name === "SecurityError") {
      return (
        action +
        " was blocked by the browser (this site may be misconfigured for passkeys)."
      );
    }
    if (name === "TypeError") {
      return "Network error — check your connection and try again.";
    }
    // A thrown Error we built from a non-OK server response carries the
    // server's own reason (e.g. a 409 "that name's taken").
    if (err && err.message) {
      return err.message;
    }
    return action + " failed. Please try again.";
  }

  /// Read a failed response's body as the thrown error's message, so the
  /// server's reason survives to `ceremony_error_message`. Falls back to a
  /// status-coded line when the body is empty/unreadable.
  async function throw_from_response(response, fallback) {
    const detail = await response.text().catch(function () {
      return "";
    });
    throw new Error(detail.trim() || fallback + " (" + response.status + ")");
  }

  /// This function gates the AUTOFILL (conditional-UI) login only. It requires
  /// conditional mediation — a capability the passive page-load autofill needs
  /// but registration does NOT. Do not reuse it to gate register (that would
  /// wrongly block passkey-capable browsers lacking conditional UI).
  async function webauthn_conditional_support() {
    if (!jsonWebAuthnSupport) {
      console.error("Webauthn functions missing");
      return false;
    }

    if (
      typeof window.PublicKeyCredential.isConditionalMediationAvailable !==
      "function"
    ) {
      console.error("Webauthn conditional mediation missing");
      return false;
    }

    try {
      if (!(await PublicKeyCredential.isConditionalMediationAvailable())) {
        console.error("Webauthn conditional mediation not availible");
        return false;
      }

      if (
        !(await PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable())
      ) {
        console.error("Webauthn platform authenticator not availible");
        return false;
      }
    } catch (e) {
      console.error("Platform checks failed with: " + e);
      return false;
    }

    return true;
  }

  /// The register-side support gate. Registration needs `parseCreationOptions-
  /// FromJSON` (line: it's called synchronously below) — nothing more. It does
  /// NOT need conditional mediation or even a platform authenticator (a roaming
  /// security key registers fine), so we hard-block ONLY on the one missing API
  /// that would otherwise throw a bare TypeError into the ceremony. Everything
  /// else is left to the ceremony + its .catch. Returns a reason string when
  /// unsupported, or null when good to go.
  function register_unsupported_reason() {
    if (!globalThis.PublicKeyCredential || !jsonWebAuthnSupport) {
      return (
        "This browser doesn't support passkeys. " +
        "Try a recent Safari (16.4+), Chrome, or Firefox."
      );
    }
    return null;
  }

  /// Attempt to authenticate using the conditional (autofill) api. Resolves to
  /// the finish response's final URL on success; THROWS on any failure (the
  /// caller's .catch renders it). Passive: it never writes success/failure UI
  /// itself.
  async function webauthn_authenticate(auth_opt_url, auth_finish_url) {
    // Forward the page's own query (?next=...) so the server-side stash also
    // lands when the login page itself was served from bfcache (no server GET
    // → login_page never stashed).
    const auth_opt_response = await fetch(
      auth_opt_url + window.location.search,
    );
    if (!auth_opt_response.ok) {
      await throw_from_response(auth_opt_response, "Could not start login");
    }

    const auth_opt_json = await auth_opt_response.json();
    const server_public_key = PublicKeyCredential.parseRequestOptionsFromJSON(
      auth_opt_json.publicKey,
    );
    var new_auth_opts = {
      mediation: auth_opt_json.mediation,
      publicKey: server_public_key,
    };

    const credential = await navigator.credentials.get(new_auth_opts);
    const auth_response_str = JSON.stringify(credential.toJSON());

    // Send the response to the server for verification.
    const finish_auth_response = await fetch(auth_finish_url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: auth_response_str,
    });

    // fetch FOLLOWS the server's success redirect, so the final status is the
    // ?next TARGET's — which can legitimately 404 (a stashed link to a page the
    // ceremony's role still can't see, or a deleted bookmark). The ceremony's
    // own verdict is the redirect: the finish handler only ever redirects on
    // success, and only ever errors without one.
    if (!finish_auth_response.redirected && !finish_auth_response.ok) {
      await throw_from_response(finish_auth_response, "Could not finish login");
    }

    // The final URL is the stashed ?next destination (Phase DE), or /.
    return finish_auth_response.url || true;
  }

  /// Attempt to register a new passkey. Resolves to the finish response's final
  /// URL on success; THROWS on any failure (the caller's .catch renders it).
  async function webauthn_register(
    start_register_url,
    finish_register_url,
    display_name,
  ) {
    const register_opt_response = await fetch(
      // encodeURIComponent so a name with ? / # % rides the path intact — a raw
      // name silently truncated (`?`) or 404'd (`/`) the registration (DM.7).
      // location.search forwards ?next= — see webauthn_authenticate.
      start_register_url +
        "/" +
        encodeURIComponent(display_name) +
        window.location.search,
      {
        method: "GET",
        headers: {
          "Content-Type": "application/json",
        },
      },
    );
    if (!register_opt_response.ok) {
      // Server's body carries the reason (e.g. a 409 "that name's taken").
      await throw_from_response(
        register_opt_response,
        "Could not start registration",
      );
    }

    const register_opt_json = await register_opt_response.json();
    const rr_publicKey = PublicKeyCredential.parseCreationOptionsFromJSON(
      register_opt_json.publicKey,
    );
    var new_rr = {
      publicKey: rr_publicKey,
    };
    const credential = await navigator.credentials.create(new_rr);
    const register_response_str = JSON.stringify(credential.toJSON());

    const finish_reg_response = await fetch(finish_register_url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: register_response_str,
    });

    // Same as authenticate: the redirect IS the success signal — the followed
    // ?next target's own status (a legit 404) must not fail the ceremony.
    if (!finish_reg_response.redirected && !finish_reg_response.ok) {
      await throw_from_response(
        finish_reg_response,
        "Could not finish registration",
      );
    }

    // The final URL carries the stashed ?next.
    return finish_reg_response.url || true;
  }

  htmx.defineExtension("webauthn-autofill", {
    onEvent: function (name, evt) {
      if (name !== "htmx:afterProcessNode") {
        return;
      }
      if (!evt.target.getAttribute("webauthn-autofill")) {
        return;
      }
      console.log("Fired Webauthn Autofill for node " + evt.detail.elt);
      webauthn_conditional_support()
        .then(function (supported) {
          if (!supported) {
            // Conditional UI unavailable — autofill is a passive enhancement,
            // so skip silently (the user can still register / other flows work).
            return null;
          }
          return webauthn_authenticate(
            "/login/get_auth_opts",
            "/login/finish_authentication",
          );
        })
        .then(function (auth) {
          if (auth) {
            // auth is the finish fetch's final URL (string) — the server's
            // ?next redirect target — or `true` from an older path.
            window.location.href = typeof auth === "string" ? auth : "/";
          }
        })
        .catch(function (err) {
          console.error("Autofill login ceremony failed: " + err);
          // Autofill is passive: a user-dismissal (NotAllowedError) is normal —
          // stay quiet. Surface only genuine failures (server/network).
          if (
            err &&
            err.name !== "NotAllowedError" &&
            err.name !== "AbortError"
          ) {
            show_error(ceremony_error_message(err, "Login"));
          }
        });
    },

    getSelectors: function () {
      return ["[webauthn-autofill]"];
    },
  });

  htmx.defineExtension("webauthn-register", {
    onEvent: function (name, evt) {
      if (name !== "htmx:beforeRequest") {
        return;
      }
      console.log("Fired Webauthn Register for node " + evt.detail.elt);
      evt.preventDefault();

      const username_field = document.getElementById("username");
      const username = username_field ? username_field.value.trim() : "";
      if (!username) {
        show_error("Please enter a username.");
        return;
      }

      const unsupported = register_unsupported_reason();
      if (unsupported) {
        show_error(unsupported);
        return;
      }

      // Clear any stale message before a fresh attempt.
      show_error("");

      webauthn_register(
        "/login/start_register",
        "/login/finish_register",
        username,
      )
        .then(function (register) {
          if (register) {
            window.location.href =
              typeof register === "string" ? register : "/";
          }
        })
        .catch(function (err) {
          console.error("Registration ceremony failed: " + err);
          show_error(ceremony_error_message(err, "Registration"));
        });
    },
  });
})();
