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

                        //if (typeof window.PublicKeyCredential.parseCreationOptionsFromJSON === 'function') {
                        //    authOptions = PublicKeyCredential.parseCreationOptionsFromJSON(authOptions);
                        //} else {
                        //Due to a Safari bug, hand create our authOptions object
                        //note we're using a fromBase64 function is not availible in Chrome yet so fingers crossed
                        //    authOptions["publicKey"]["challenge"] = Uint8Array.fromBase64(authOptions["publicKey"]["challenge"], { alphabet: 'base64url' });
                        //    authOptions["publicKey"]["userVerification"] = "preferred";
                        //}

                        //const webAuthnResponse = await navigator.credentials.get(authOptions);


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