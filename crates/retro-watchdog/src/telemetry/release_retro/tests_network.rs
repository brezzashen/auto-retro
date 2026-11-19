//! Canary failures on the path to Google Cloud classify as `gcp_network`, not as the beta cluster.

use super::*;

#[test]
fn canary_tails_of_2026_11_11_classify_as_gcp_network() {
    let cases = [
        ("  ✗ prepared by another session, which failed: Failed with exit code 1: kubectl --context gke_example-project_us-central1_app-gke -n app-canary get deploy api crm agent --ignore-not-found -o json\nUnable to connect to the server: net/http: TLS handshake timeout\n", "gcp_network"),
        ("  ✗ error sending request for url (https://canary.example.com/api/health)\nerror: script \"canary\" exited with code 1", "gcp_network"),
        ("  ✗ submit failed: Uploading local file [/var/x/upload.tgz] to [gs://b/source/1.tgz].\nERROR: (gcloud.builds.submit) 400 Could not upload file [/var/x/upload.tgz] to [b/source/1.tgz]", "gcp_network"),
        ("  ✗ submit failed: gcloud builds submit timed out 2 times after 480 s each and created no cloud build", "gcp_network"),
    ];
    for (tail, class) in cases { assert_eq!(stats::classify(tail), class, "{tail}"); }
}

#[test]
fn a_local_cluster_tls_timeout_stays_beta_cluster() {
    assert_eq!(stats::classify("Unable to connect to the server: net/http: TLS handshake timeout"), "beta_cluster");
    assert_eq!(stats::classify("kubectl --context docker-desktop -n mars-crm apply: net/http: TLS handshake timeout"), "beta_cluster");
}
