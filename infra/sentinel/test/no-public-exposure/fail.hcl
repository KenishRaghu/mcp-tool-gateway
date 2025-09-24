import "module" "common" {
  source = "../../common-functions.sentinel"
}

mock "tfplan/v2" {
  module {
    source = "../../mocks/tfplan-insecure.sentinel"
  }
}

test {
  rules = {
    main = false
  }
}
