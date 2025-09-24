import "module" "common" {
  source = "../../common-functions.sentinel"
}

mock "tfplan/v2" {
  module {
    source = "../../mocks/tfplan-production.sentinel"
  }
}

test {
  rules = {
    main = true
  }
}
