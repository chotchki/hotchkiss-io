"use strict";
(function () {
    const polyfill_webauthn = window.webauthnJSON;

    /// This function is used to ensure browser functionality exists, do not call the other functions without it returning true
    async function webauthn_conditional_support() {
        if (!polyfill_webauthn.supported()) {
            console.error("Webauthn functions missing");
            return false;
        }

        if (typeof window.PublicKeyCredential.isConditionalMediationAvailable !== 'function') {
            console.error("Webauthn conditional mediation missing");
            return false;
        }

        if (!await PublicKeyCredential.isConditionalMediationAvailable()) {
            console.error("Webauthn conditional mediation not availible");
            return false;
        }

        if (!await PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable()) {
            console.error("Webauthn platform authenticator not availible");
            return false;
        }

        return true;
    }

    /// Attempt to authenticate using the conditional api
    async function webauthn_authenticate(auth_opt_url, auth_finish_url) {
        const auth_opt_response = await fetch(auth_opt_url);
        if (!auth_opt_response.ok) {
            console.error("Response from auth options: ${response.status}");
            return false;
        }

        let auth_opt_json = await response.json();

        //Due to a Safari bug, having to use a ponyfill
        const auth_response = await (await polyfill_webauthn.get(auth_opt_json));
        const auth_response_str = JSON.stringify(auth_response);

        // Send the response to your server for verification and
        // authenticate the user if the response is valid.
        const finish_auth_response = await fetch(auth_finish_url, {
            method: "POST",
            headers: {
                "Content-Type": "application/json",
            },
            body: auth_response_str
        });

        if (!finish_auth_response.ok) {
            console.error("Response from auth options: ${finish_auth_response.status}");
            return false;
        }

        return true;
    }

    /// Attempt to authenticate using the conditional api
    async function webauthn_register(start_register_url, finish_register_url, display_name) {
        const register_opt_response = await fetch(start_register_url, {
            method: "POST",
            headers: {
                "Content-Type": "application/json",
            },
            body: JSON.stringify(display_name)
        });
        if (!register_opt_response.ok) {
            console.error('Response from start registration: ${register_opt_response.status}');
            return false;
        }

        const register_opt_json = await register_opt_response.json();

        //Due to a Safari bug, having to use a ponyfill
        const register_response = await polyfill_webauthn.create(register_opt_json);
        const register_response_str = JSON.stringify(register_response);

        const finish_reg_response = await fetch(finish_register_url, {
            method: "POST",
            headers: {
                "Content-Type": "application/json",
            },
            body: register_response_str
        });

        if (!finish_reg_response.ok) {
            console.error("Response from finish registration: ${finish_reg_response.status}");
            return false;
        }

        return true;
    }

    htmx.defineExtension('webauthn-autofill', {
        init: function (api) {
        }
    });
})();

htmx.defineExtension('webauthn-autofill', {
    init: function (api) {
        console.log("Fired Webauthn Autofill check");
        (async () => {
            if (
                typeof window.PublicKeyCredential !== 'undefined'
                && typeof window.PublicKeyCredential.isConditionalMediationAvailable === 'function'
            ) {
                const available = await PublicKeyCredential.isConditionalMediationAvailable();

                if (available) {
                    try {
                        // Retrieve authentication options for `navigator.credentials.get()`
                        // from your server.
                        const response = await fetch("/login/getAuthOptions");
                        if (!response.ok) {
                            throw new Error('Response from auth options: ${response.status}');
                        }

                        let authOptions = await response.json();

                        //Due to a Safari bug, having to use a ponyfill
                        const polyfill_webauthn = window.webauthnJSON;
                        const authResponse = await (await polyfill_webauthn.get(authOptions));
                        const authResponseJson = JSON.stringify(authResponse);

                        // Send the response to your server for verification and
                        // authenticate the user if the response is valid.
                        const finish_auth = await fetch("/login/finish_authentication", {
                            method: "POST",
                            headers: {
                                "Content-Type": "application/json",
                            },
                            body: authResponseJson
                        });

                        if (!finish_auth.ok && !finish_auth.redirected) {
                            console.error("Response from auth options: ${finish_auth.status}");
                            return;
                        }

                        console.log("Success!")
                    } catch (err) {
                        console.error('Error with conditional UI:', err);
                    }
                }
            }
        })();
    }
});

htmx.defineExtension('webauthn-register', {
    onEvent: function (name, evt) {
        if (name !== "htmx:beforeRequest") {
            return;
        }
        console.log("Fired Webauthn Register");
        evt.preventDefault();
        (async () => {
            if (!PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable()) {
                console.error("No platform authenticator availible!");
                return;
            }

            const response = await fetch("/login/register/" + document.getElementById("username").value);
            if (!response.ok) {
                console.error('Response from auth options: ${response.status}');
            }

            const registerOptions = await response.json();
            //Due to a Safari bug, having to use a ponyfill
            const polyfill_webauthn = window.webauthnJSON;
            const registerResponse = await polyfill_webauthn.create(registerOptions);
            const registerResponseJson = JSON.stringify(registerResponse);

            const finish_response = await fetch("/login/finish_register", {
                method: "POST",
                headers: {
                    "Content-Type": "application/json",
                },
                body: registerResponseJson
            });

            if (!finish_response.ok && !finish_response.redirected) {
                console.error("Response from auth options: ${finish_response.status}");
                return;
            }

            console.log("Success!");
        })();
    }
});