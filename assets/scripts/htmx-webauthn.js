"use strict";
(function () {
  const jsonWebAuthnSupport =
    !!globalThis.PublicKeyCredential?.parseCreationOptionsFromJSON;

  /// This function is used to ensure browser functionality exists, do not call the other functions without it returning true
  async function webauthn_conditional_support() {
    console.log("Performing conditional checks");

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

  /// Attempt to authenticate using the conditional api
  async function webauthn_authenticate(auth_opt_url, auth_finish_url) {
    console.log("Calling webauthn_authenticate");
    // Forward the page's own query (?next=...) so the server-side stash also
    // lands when the login page itself was served from bfcache (no server GET
    // → login_page never stashed).
    const auth_opt_response = await fetch(
      auth_opt_url + window.location.search,
    );
    if (!auth_opt_response.ok) {
      console.error(`Response from auth options: ${response.status}`);
      return false;
    } else {
      console.log("Got past the opt call");
    }

    const auth_opt_json = await auth_opt_response.json();

    console.log("parsing server auth");
    const server_public_key = PublicKeyCredential.parseRequestOptionsFromJSON(
      auth_opt_json.publicKey,
    );
    var new_auth_opts = {
      mediation: auth_opt_json.mediation,
      publicKey: server_public_key,
    };

    console.log("prompting for autofill");
    const credential = await navigator.credentials.get(new_auth_opts);
    const auth_response_str = JSON.stringify(credential.toJSON());

    console.log("returned from conditional prompt, sending to server");
    // Send the response to your server for verification and
    // authenticate the user if the response is valid.
    const finish_auth_response = await fetch(auth_finish_url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: auth_response_str,
    });

    // fetch FOLLOWS the server's success redirect, so the final status is the
    // ?next TARGET's — which can legitimately 404 (a stashed link to a page
    // the ceremony's role still can't see, or a deleted bookmark). The
    // ceremony's own verdict is the redirect: the finish handler only ever
    // redirects on success, and only ever errors without one.
    if (!finish_auth_response.redirected && !finish_auth_response.ok) {
      console.error(
        `Response from finish authentication: ${finish_auth_response.status}`,
      );
      return false;
    }

    console.log("server response was good");
    // The final URL is the stashed ?next destination (Phase DE), or /.
    return finish_auth_response.url || true;
  }

  /// Attempt to authenticate using the conditional api
  async function webauthn_register(
    start_register_url,
    finish_register_url,
    display_name,
  ) {
    const register_opt_response = await fetch(
      // location.search forwards ?next= — see webauthn_authenticate.
      start_register_url + "/" + display_name + window.location.search,
      {
        method: "GET",
        headers: {
          "Content-Type": "application/json",
        },
      },
    );
    if (!register_opt_response.ok) {
      console.error(
        `Response from start registration: ${register_opt_response.status}`,
      );
      return false;
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
      console.error(
        `Response from finish registration: ${finish_reg_response.status}`,
      );
      return false;
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
        .then(() => {
          console.log("Firing post conditional support check");
          return webauthn_authenticate(
            "/login/get_auth_opts",
            "/login/finish_authentication",
          );
        })
        .then((auth) => {
          if (auth) {
            // auth is the finish fetch's final URL (string) — the server's
            // ?next redirect target — or `true` from an older path; both land
            // somewhere sane.
            window.location.href = typeof auth === "string" ? auth : "/";
          } else {
            document.getElementById("error_message").innerHTML =
              "Error logging in";
          }
        })
        .catch((err) => {
          console.error("Had a problem " + err);
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

      const username = document.getElementById("username").value;

      webauthn_conditional_support()
        .then(() =>
          webauthn_register(
            "/login/start_register",
            "/login/finish_register",
            username,
          ),
        )
        .then((register) => {
          if (register) {
            window.location.href =
              typeof register === "string" ? register : "/";
          } else {
            document.getElementById("error_message").innerHTML =
              "Error registering";
          }
        });
    },
  });
})();
